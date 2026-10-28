use crate::cat::{self, Cat, Trick};
use crate::lyrics;
use crate::media::{self, LyricState, Media};
use crate::pomodoro::{Mode, Pomodoro};
use crate::beat::Beat;
use crate::peek::{self, Peek};
use crate::pet::{self, Food, Pet};
use crate::status::{self, Capture, Net, Power};
use crate::shelf::{self, Shelf};
use crate::store;
use crate::todo::{Task, Todo};
use crate::sys::{animations_enabled, chime, local_time};
use crate::ui::{Align, Color, Font, Image, Painter, Rect, rgba};
use crate::water::{self, Prompt, Water};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub const CANVAS_W: f32 = 800.0;
pub const CANVAS_H: f32 = 170.0;
const OPEN_W: f32 = 420.0;
/// The panel widens for lyrics: player on the left, scrolling lines on the right.
const LYRICS_W: f32 = 520.0;
const OPEN_H: f32 = 140.0;
const PILL_H: f32 = 30.0;
/// Canvas of the window the cat naps in when it strays off the notch.
pub const STRAY_W: f32 = 76.0;
pub const STRAY_H: f32 = 48.0;
/// The collapsed pill while it carries a lyric line: at least this wide, growing to fit
/// the song's longest line up to the max.
const LYRIC_PILL_W: f32 = 420.0;
const LYRIC_PILL_MAX: f32 = CANVAS_W - 2.0 * SHOULDER - 20.0;
/// Room the cat and the clock take beside the lyric line.
const LYRIC_PILL_PAD: f32 = 112.0;
/// One lyric line rolling over to the next in the pill.
const LINE_SECS: f32 = 0.42;
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
// Glow on the cat's headset mic, phone-style: orange for the mic, green for the camera.
const MIC: Color = rgba(1.0, 0.62, 0.04, 1.0);
const CAM: Color = rgba(0.2, 0.84, 0.29, 1.0);
/// The music outline and its lights.
const GLOW: Color = rgba(1.0, 1.0, 1.0, 1.0);
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
/// A toast's content fades in while the pill grows, and fades out before it shrinks back.
const TOAST_IN: f32 = 0.24;
const TOAST_OUT: f32 = 0.18;
const TODO_ROWS: usize = 3;

#[derive(Clone, Copy, PartialEq)]
pub enum Tab {
    Water,
    Focus,
    Todo,
    Shelf,
    Music,
}

#[derive(Debug, Clone, PartialEq)]
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
    /// Shown since; the content fades and rises in from here.
    at: Instant,
    until: Instant,
    /// A new song: shown centred as "title | artist", without the status dot.
    song: bool,
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
    let t = local_time();
    Clock {
        day: t.year * 10000 + t.month * 100 + t.day,
        hour: t.hour as usize,
        minute: (t.hour * 60 + t.minute) as u16,
        now: (t.hour * 60 + t.minute) as f32 + t.second as f32 / 60.0,
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

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub struct App {
    pub water: Water,
    /// How fed the cat is; sips, finished tasks and Focus sessions feed it.
    pet: Pet,
    pub pomo: Pomodoro,
    pub cat: bool,
    pub compact: bool,
    tab: Tab,
    water_settings: bool,
    reset_armed: Option<Instant>,
    alert: Option<Alert>,
    /// Task reminders that came due while another alert was showing.
    queued: VecDeque<Alert>,
    /// Water nudge held back until the running Focus session ends.
    held_water: Option<Prompt>,
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
    capture: Option<Capture>,
    caps: Option<bool>,
    ticks: u32,
    pub shelf: Shelf,
    shelf_scroll: usize,
    /// Something is being dragged over the notch.
    pub drag_hover: bool,
    pub media: Option<Media>,
    art: Option<(u64, Image, Color)>,
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
    /// The lyrics button was pressed: the player morphs between layouts until `wide` settles.
    /// Page turns leave it off, so they slide the finished lyrics view instead.
    ly_morph: bool,
    /// The slider under a held mouse button, with its hit area to map the cursor onto.
    scrub: Option<(Slider, Rect)>,
    /// Where the progress bar is being dragged to; sent as a seek on release.
    seek_to: Option<Duration>,
    volume: Option<media::Volume>,
    /// Output level for the beat pulse, reopened now and then to follow device switches.
    meter: Option<media::Meter>,
    meter_at: Option<Instant>,
    /// The colour under the cursor while the picker hotkey is held.
    picking: Option<(u8, u8, u8)>,
    /// 0 with the cat home in the notch, 1 once it has moved to its nap spot; eased both ways.
    stray_fade: f32,
    /// Outline lights: how far faded in, their clock (seconds, sped up by loud music), and a
    /// slow-moving loudness that nudges their speed.
    lights: f32,
    lights_t: f32,
    energy: f32,
    beat: Beat,
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
    shelf_cache: HashMap<PathBuf, (String, Option<Image>)>,
    /// When each just-dropped item starts falling in.
    shelf_added: Vec<(PathBuf, Instant)>,
    /// Closed pill size as drawn, gliding toward `pill_width()` x `pill_height()`.
    pill_size: (f32, f32),
    /// Lyric line in the pill and the one it replaced, rolling over since the instant.
    pill_line: (String, String, Instant),
    /// Lyric pill width fitted to the current song's longest line, keyed by song.
    pill_fit: (String, f32),
    /// Whether the notch should be on screen; false while hidden over a full-screen app.
    pub shown: bool,
    /// 0 = slid up above the screen edge, 1 = in place.
    peek: f32,
    peek_vel: f32,
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
            queued: VecDeque::new(),
            held_water: None,
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
            capture: None,
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
            ly_morph: false,
            scrub: None,
            seek_to: None,
            volume: media::Volume::open(),
            meter: None,
            meter_at: None,
            picking: None,
            stray_fade: 0.0,
            lights: 0.0,
            lights_t: 0.0,
            energy: 0.0,
            beat: Beat::default(),
            ly_pos: 0.0,
            ly_vel: 0.0,
            ly_target: 0.0,
            ly_heights: (String::new(), Vec::new()),
            drag_shown: 0.0,
            drag_since: Instant::now(),
            shelf_cache: HashMap::new(),
            shelf_added: Vec::new(),
            pill_size: (0.0, PILL_H),
            pill_line: (String::new(), String::new(), Instant::now()),
            pill_fit: (String::new(), LYRIC_PILL_W),
            shown: true,
            peek: 1.0,
            peek_vel: 0.0,
            pet: {
                let c = clock();
                Pet::from_text(&std::fs::read_to_string(store::dir().join(pet::FILE)).unwrap_or_default(), c.day, c.minute)
            },
        };
        app.pill_size.0 = app.pill_width();
        app.water.new_day(clock().day);
        app
    }

    pub fn save_settings(&self) {
        let _ = store::save(SETTINGS, &format!("cat={}\ncompact={}\n", self.cat as u8, self.compact as u8));
    }

    fn save_pet(&self) {
        let _ = store::save(pet::FILE, &self.pet.to_text());
    }

    /// Feeds the cat; a sip or Focus session gets a purr (a finished task already cheers).
    fn feed(&mut self, food: Food) {
        let good = self.pet.feed(food);
        self.save_pet();
        if self.cat && food != Food::Task {
            self.mascot.react(cat::Trick::Purr, (0.0, 0.0), Instant::now());
        }
        if good {
            let gift = match self.pet.accessory() {
                pet::Accessory::Scarf => "a scarf",
                _ => "a bell",
            };
            self.toast("Good day!".into(), format!("The cat earned {gift}"), TASK);
        }
    }

    fn save_water(&self) {
        let _ = store::save(water::FILE, &self.water.to_text());
    }

    pub fn open(&self) -> bool {
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
            if let Some(t) = &mut self.toast {
                t.song = true;
            }
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
                self.land(paths);
            }
        }
    }

    /// A screenshot from the clipboard joins the shelf, and the cat cheers for the catch.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn catch_screenshot(&mut self, path: PathBuf) {
        self.land(vec![path]);
        self.toast("Screenshot caught".into(), "Drag it out of the Shelf".into(), WATER);
        if self.cat {
            self.mascot.react(cat::Trick::Cheer, (0.0, 0.0), Instant::now());
        }
    }

    fn land(&mut self, paths: Vec<PathBuf>) {
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

    /// Changes the showing toast in place, so live updates (the colour picker) don't replay its fade-in.
    fn retoast(&mut self, title: String, detail: String, color: Color) {
        match &mut self.toast {
            Some(t) if !t.song => {
                (t.title, t.detail, t.color) = (title, detail, color);
                t.until = Instant::now() + Duration::from_secs(TOAST_SECS);
            }
            _ => self.toast(title, detail, color),
        }
    }

    fn toast(&mut self, title: String, detail: String, color: Color) {
        let now = Instant::now();
        self.toast = Some(Toast { title, detail, color, at: now, until: now + Duration::from_secs(TOAST_SECS), song: false });
    }

    /// Copied text that is a colour, link or sum gets a peek in the pill; true when one shows.
    // shortcut: Linux has no clipboard listener yet, so only Windows calls this; add wl-paste --watch if asked.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn peek_clipboard(&mut self, text: &str) -> bool {
        let Some(found) = peek::classify(text) else { return false };
        match found {
            Peek::Color(r, g, b) => {
                let swatch = rgba(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0);
                self.toast(peek::rgb_hex(r, g, b), format!("rgb({r}, {g}, {b})"), swatch);
            }
            Peek::Link(host) => self.toast("Link copied".into(), host, WATER),
            Peek::Sum(answer) => {
                let expr: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
                let expr = if expr.chars().count() > 16 { format!("{}…", expr.chars().take(15).collect::<String>()) } else { expr };
                self.toast(expr, format!("= {answer}"), TASK);
            }
        }
        true
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn start_pick(&mut self) {
        self.picking = Some((0, 0, 0));
    }

    pub fn picking(&self) -> bool {
        self.picking.is_some()
    }

    /// The cat's eyes and the pill show the colour under the cursor as it moves.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn pick_at(&mut self, (r, g, b): (u8, u8, u8)) {
        if self.picking == Some((r, g, b)) && self.toast.is_some() {
            return;
        }
        self.picking = Some((r, g, b));
        let c = rgba(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0);
        self.mascot.tint = Some(c);
        self.retoast(peek::rgb_hex(r, g, b), "Release to copy".into(), c);
    }

    /// Ends picking; returns the hex code to copy.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn finish_pick(&mut self) -> Option<String> {
        let (r, g, b) = self.picking.take()?;
        self.mascot.tint = None;
        let hex = peek::rgb_hex(r, g, b);
        self.retoast(hex.clone(), "Copied".into(), rgba(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0));
        Some(hex)
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
            let capture = status::capture();
            if let Some((title, detail)) = self.capture.as_ref().and_then(|old| status::capture_message(old, &capture)) {
                self.toast(title, detail, if capture.cam.is_some() { CAM } else { MIC });
            }
            self.capture = Some(capture);
            self.mascot.headset = self.capture_color();
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
        // Ends on the frame it finishes fading, not at the next 1 s tick.
        if self.toast.as_ref().is_some_and(|t| now >= t.until) {
            self.toast = None;
        }
        if self.page_from.is_some_and(|(_, _, at)| at.elapsed().as_secs_f32() >= PAGE_SECS) {
            self.page_from = None;
        }
        if self.cat {
            self.mascot.step(now, dt);
        }
        if self.grooving() {
            if self.meter_at.is_none_or(|at| at.elapsed() > Duration::from_secs(10)) {
                (self.meter, self.meter_at) = (media::Meter::open(), Some(now));
            }
            let peak = self.meter.as_ref().map_or(0.0, |m| m.peak());
            if self.beat.step(peak, dt) && self.cat && !self.reduced_motion {
                self.mascot.bob(now);
            }
        } else {
            (self.beat.pulse, self.beat.level) = (0.0, 0.0);
        }
        let on = self.grooving() as u8 as f32;
        // Quick to light up when a song starts, gentler going out.
        self.lights = if on > self.lights { (self.lights + dt / 0.2).min(on) } else { (self.lights - dt / 0.4).max(on) };
        if self.lights > 0.0 {
            // Speed follows the music gently; brightness never does, so nothing pulses.
            self.energy += (self.beat.level - self.energy) * (dt * 1.5).min(1.0);
            let pace = if self.reduced_motion { 0.0 } else { 1.0 + 1.8 * self.energy.min(1.0) };
            // Every light finishes a whole number of laps in 100 s, so wrapping there never jumps.
            self.lights_t = (self.lights_t + dt * pace) % 100.0;
        }
        let away = self.mascot.away as u8 as f32;
        self.stray_fade = if self.reduced_motion {
            away
        } else if away > self.stray_fade {
            (self.stray_fade + dt / 0.45).min(away)
        } else {
            (self.stray_fade - dt / 0.35).max(away)
        };
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
        if self.wide == wide {
            self.ly_morph = false;
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
        // Grows with an ease-out, shrinks a little quicker: leaving should never feel like waiting.
        let (pw, ph) = (self.pill_width(), self.pill_height());
        if self.reduced_motion {
            self.pill_size = (pw, ph);
        }
        let rate = if pw < self.pill_size.0 { 16.0 } else { 11.0 };
        let glide = |v: f32, to: f32| if (to - v).abs() < 0.3 { to } else { approach(v, to, dt, rate) };
        self.pill_size = (glide(self.pill_size.0, pw), glide(self.pill_size.1, ph));
        // Drops in softly, leaves quicker; clamped so it never dips below its resting place.
        let shown = self.shown as u8 as f32;
        if self.reduced_motion {
            (self.peek, self.peek_vel) = (shown, 0.0);
        }
        let omega: f32 = if self.shown { 20.0 } else { 30.0 };
        let mut left = dt;
        while left > 0.0 {
            let h = left.min(0.002);
            self.peek_vel += (omega * omega * (shown - self.peek) - 2.0 * omega * self.peek_vel) * h;
            self.peek = (self.peek + self.peek_vel * h).clamp(0.0, 1.0);
            left -= h;
        }
        if (shown - self.peek).abs() < 0.003 && self.peek_vel.abs() < 0.05 {
            (self.peek, self.peek_vel) = (shown, 0.0);
        }
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
            || self.pill_size != (self.pill_width(), self.pill_height())
            || self.pill_line.2.elapsed().as_secs_f32() < LINE_SECS
            // The notes in a blank lyric pill keep waving.
            || (self.shown && self.pill_lyric().is_some_and(|l| l.is_empty()))
            || self.peek != self.shown as u8 as f32
            || self.peek_vel != 0.0
            || self.grooving()
            || self.toast_moving()
            || self.lights != self.grooving() as u8 as f32
            || self.stray_fade != self.mascot.away as u8 as f32
            || self.picking.is_some()
    }

    /// Music playing behind the closed pill: the outline lights up from the first moment, toasts included.
    fn grooving(&self) -> bool {
        self.shown && self.anim == 0.0 && self.alert.is_none() && self.music_playing()
    }

    fn music_playing(&self) -> bool {
        let Some(media) = &self.media else { return false };
        media.shared.lock().is_ok_and(|s| s.track.as_ref().is_some_and(|t| t.playing))
    }

    /// Toast content opacity and vertical offset: rises in, then fades out just before it ends.
    fn toast_fade(&self) -> (f32, f32) {
        let Some(t) = &self.toast else { return (1.0, 0.0) };
        let now = Instant::now();
        let k = (now.saturating_duration_since(t.at).as_secs_f32() / TOAST_IN).min(1.0);
        let k = 1.0 - (1.0 - k).powi(3);
        let out = (t.until.saturating_duration_since(now).as_secs_f32() / TOAST_OUT).min(1.0);
        let rise = if self.reduced_motion { 0.0 } else { 5.0 * (1.0 - k) };
        (k.min(out), rise)
    }

    /// The toast is still fading in or out, so frames must keep coming.
    fn toast_moving(&self) -> bool {
        let now = Instant::now();
        self.toast.as_ref().is_some_and(|t| {
            now.saturating_duration_since(t.at).as_secs_f32() < TOAST_IN || t.until.saturating_duration_since(now).as_secs_f32() < TOAST_OUT + 0.05
        })
    }

    /// How visible the outline lights are: eased in while music plays, out when it stops,
    /// and gone once the panel opens.
    fn glow(&self) -> f32 {
        let k = self.lights;
        k * k * (3.0 - 2.0 * k) * (1.0 - self.anim.clamp(0.0, 1.0))
    }

    /// Thin, soft white lights wander the outline both ways at their own speeds.
    fn draw_lights(&self, p: &Painter, top: f32, g: f32) {
        // (laps per second, direction, length in DIPs, start)
        const LIGHTS: [(f32, f32, f32, f32); 4] = [(0.16, 1.0, 54.0, 0.0), (0.11, -1.0, 38.0, 0.3), (0.22, 1.0, 26.0, 0.62), (0.07, -1.0, 70.0, 0.85)];
        const SEGS: usize = 16;
        let (x, w, h) = self.panel();
        let r = lerp(self.pill_size.1 / 2.0, 24.0, self.anim.clamp(0.0, 1.0));
        // Centred on the 1 DIP border just outside the black shape.
        let d = 0.5;
        let len = outline_len(w, h, r, d, top);
        // Lights fade at the outline's two ends, where it meets the screen edge.
        let ends = |s: f32| (s.min(len - s) / 10.0).clamp(0.0, 1.0);
        for (speed, dir, span, start) in LIGHTS {
            // Each lap runs a light's length past the end, so it leaves fully before it comes round again.
            let run = (start + self.lights_t * speed).fract() * (len + span);
            let head = if dir > 0.0 { run } else { len - run };
            for i in 0..SEGS {
                let (f0, f1) = (i as f32 / SEGS as f32, (i + 1) as f32 / SEGS as f32);
                let (s0, s1) = ((head - dir * span * f0).clamp(0.0, len), (head - dir * span * f1).clamp(0.0, len));
                if (s0 - s1).abs() < 0.01 {
                    continue;
                }
                // Soft at both ends, no bright head: just light sliding along the border.
                let bell = (std::f32::consts::PI * (f0 + f1) / 2.0).sin();
                let a = 0.8 * bell * bell * g * ends(s0);
                let (x0, y0) = outline_at(x, w, h, r, d, top, s0);
                let (x1, y1) = outline_at(x, w, h, r, d, top, s1);
                p.line(x0, y0, x1, y1, Color { a, ..GLOW }, 1.0);
            }
        }
    }

    pub fn peek(&self) -> f32 {
        self.peek
    }

    /// Polled often; returns true when the pill's lyric line changed and should roll over.
    pub fn poll_pill_lyric(&mut self) -> bool {
        let line = self.pill_lyric().unwrap_or_default();
        if line == self.pill_line.0 {
            return false;
        }
        let old = std::mem::replace(&mut self.pill_line.0, line);
        self.pill_line.1 = old;
        self.pill_line.2 = Instant::now();
        self.reduced_motion = !animations_enabled();
        true
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
        let lyric = self.pill_lyric().unwrap_or_default();
        let rec = (self.capture.as_ref().map(|c| (c.mic.is_some(), c.cam.is_some())), self.mascot.grumpy, self.mascot.accessory);
        format!("{label}|{toast}|{}|{:?}|{}|{}|{rec:?}|{lyric}", self.water.taken, self.mascot.mood, self.cat, self.compact)
    }

    /// Area the notch covers right now, shoulders included, for partial repaints.
    pub fn bounds(&self) -> Rect {
        let (x, w, h) = self.panel();
        // Room for the beat swell, which grows the shape but not the layout.
        Rect::new(x - SHOULDER - 3.0, 0.0, w + 2.0 * SHOULDER + 6.0, h + 2.0)
    }

    /// Cat head centre in canvas DIPs, used to aim its eyes at the cursor.
    pub fn cat_pos(&self) -> (f32, f32) {
        let (x, y, _) = self.cat_place();
        (x, y)
    }

    /// The cat walks from the pill's left end to the panel's top-left corner as it opens.
    fn cat_place(&self) -> (f32, f32, f32) {
        let t = self.anim.clamp(0.0, 1.0);
        let pill_x = (CANVAS_W - self.pill_size.0) / 2.0 + 22.0;
        let open_x = (CANVAS_W - self.open_w()) / 2.0 + 32.0;
        // A small swell mid-journey so it reads as a hop, not a slide.
        let s = 0.7 * (1.0 + 0.18 * (t * std::f32::consts::PI).sin());
        (lerp(pill_x, open_x, t), lerp(self.pill_size.1 / 2.0 + 2.0, 19.0, t), s)
    }

    fn pill_width(&self) -> f32 {
        match (&self.toast, self.compact) {
            (Some(_), _) => 320.0,
            (None, true) => 120.0,
            // Fixed width while lyrics run, so the notch doesn't jump with every line.
            (None, false) if self.pill_lyric().is_some() => self.pill_fit.1,
            (None, false) => 150.0,
        }
    }

    /// A touch taller while it carries a lyric line.
    fn pill_height(&self) -> f32 {
        if self.pill_lyric().is_some() { 36.0 } else { PILL_H }
    }

    /// Line being sung, shown between the cat and the clock while collapsed with lyrics on.
    /// Empty when there is no line to show (the notes wave then); None when paused.
    fn pill_lyric(&self) -> Option<String> {
        if !self.show_lyrics || self.compact || self.toast.is_some() {
            return None;
        }
        // Read in place: snapshot() would clone every lyric line, and this runs several times a frame.
        let snap = self.media.as_ref()?.shared.lock().ok()?;
        let track = snap.track.as_ref().filter(|t| t.playing)?;
        // No line to sing (lyrics missing, or before the first line): the notes wave instead.
        let LyricState::Found(lines) = &snap.lyrics else { return Some(String::new()) };
        Some(lyrics::current(lines, track.position()).map_or(String::new(), |i| lines[i].1.clone()))
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
            self.held_water = Some(p);
        }
        if !(self.pomo.running() && self.pomo.mode == Mode::Focus) {
            if let Some(p) = self.held_water.take() {
                self.raise(Alert::Water(p));
            }
        }
        if let Some(next) = self.pomo.tick(Instant::now()) {
            // A break starting means a Focus session just finished.
            if next != Mode::Focus {
                self.feed(Food::Focus);
            }
            self.raise(Alert::Pomodoro(next));
        }
        if self.reset_armed.is_some_and(|t| t.elapsed() > Duration::from_secs(3)) {
            self.reset_armed = None;
        }
        if let Some(title) = self.todo.due(c.day, c.minute) {
            self.todo.save();
            self.raise(Alert::Task(title));
        }
        if self.alert.is_none() {
            if let Some(next) = self.queued.pop_front() {
                self.raise(next);
            }
        }
        let day = self.pet.day;
        self.pet.update(c.day, c.minute);
        if day != self.pet.day || self.ticks.is_multiple_of(600) {
            self.save_pet();
        }
        self.mascot.grumpy = self.pet.grumpy();
        self.mascot.accessory = self.pet.accessory();
        self.ticks = self.ticks.wrapping_add(1);
        self.poll_status();
        if self.toast.as_ref().is_some_and(|t| Instant::now() >= t.until) {
            self.toast = None;
        }
        let open = self.open();
        self.mascot.idle(idle_ms, open, Instant::now());
    }

    fn raise(&mut self, alert: Alert) {
        if place_alert(&mut self.alert, &mut self.queued, alert) {
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
        let now = Instant::now();
        let hit = self.hits.iter().rev().find(|(r, _)| r.contains(x, y)).map(|&(_, a)| a);
        if self.cat {
            let (cx, cy) = self.cat_pos();
            let reaction = match hit {
                // Clicking the cat itself pets it.
                None if (x - cx).hypot(y - cy) < 16.0 => Some(Trick::Purr),
                None => None,
                Some(Action::Sip | Action::PomoStartNext) => Some(Trick::Cheer),
                Some(Action::PomoToggle) if !self.pomo.running() => Some(Trick::Cheer),
                Some(Action::TodoToggle(i)) if self.todo.tasks.get(i).is_some_and(|t| !t.done) => Some(Trick::Cheer),
                Some(Action::TodoDelete(_) | Action::ShelfRemove(_) | Action::ShelfClear | Action::Reset) => Some(Trick::Startle),
                Some(Action::Media(media::Cmd::Next | media::Cmd::Prev)) => Some(Trick::Groove),
                Some(_) => Some(Trick::Peek),
            };
            if let Some(r) = reaction {
                self.mascot.react(r, cat::gaze(x - cx, y - cy), now);
            }
        }
        let Some(action) = hit else { return };
        let c = clock();
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
            Action::Lyrics => {
                self.show_lyrics = !self.show_lyrics;
                self.ly_morph = true;
            }
            Action::WaterChart => self.water_chart = !self.water_chart,
            Action::Sip => {
                self.water.sip(c.hour);
                self.feed(Food::Sip);
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
                    let done = t.done;
                    self.todo.save();
                    if done {
                        self.feed(Food::Task);
                    }
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
        let w = lerp(self.pill_size.0, self.open_w(), t);
        ((CANVAS_W - w) / 2.0, w, lerp(self.pill_size.1, OPEN_H, t))
    }

    pub fn draw(&mut self, p: &Painter) {
        self.hits.clear();
        let t = self.anim.clamp(0.0, 1.0);
        let (x, w, h) = self.panel();
        let radius = lerp(self.pill_size.1 / 2.0, 24.0, t);
        // Start above the screen edge so only the bottom corners look rounded.
        let shape = Rect::new(x, -radius, w, h + radius);
        let g = self.glow();
        let lit = g > 0.01;
        if lit {
            // The outline glows steadily white: a soft halo under a clear 1 DIP ring.
            let c = GLOW;
            let ring = |d: f32, a: f32| p.round(Rect::new(x - d, -radius, w + 2.0 * d, h + radius + d), radius + d, Color { a: a * g, ..c });
            ring(3.0, 0.12);
            ring(1.0, 0.5);
        }
        p.round(shape, radius, BG);
        let shoulder = lerp(8.0, SHOULDER, t);
        p.shoulders(x, w, shoulder, BG);
        if lit {
            self.draw_lights(p, shoulder, g);
        }
        let open_x = (CANVAS_W - self.open_w()) / 2.0;
        if t == 0.0 {
            // Clipped so text laid out for the final size stays inside while the pill resizes.
            p.push_clip(shape, radius, 1.0);
            self.draw_pill(p, x, w);
            p.pop_clip();
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
        let gone = self.stray_ease();
        if (!self.compact || t > 0.6) && gone < 1.0 {
            let (cx, cy, s) = self.cat_place();
            // Leaving, the cat lifts and shrinks a little as it fades; returning plays that backwards.
            let fading = gone > 0.0;
            if fading {
                p.push_clip(Rect::new(0.0, 0.0, CANVAS_W, CANVAS_H), 0.0, 1.0 - gone);
            }
            // The waving paw rises from below, so keep it inside the notch.
            let clip = self.cat && self.mascot.waving();
            if clip {
                p.push_clip(shape, radius, 1.0);
            }
            self.draw_cat(p, cx, cy - 4.0 * gone, s * (1.0 - 0.25 * gone));
            if clip {
                p.pop_clip();
            }
            if fading {
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

    /// Smoothstep of `stray_fade`: how far the cat has moved from the notch to its nap spot.
    pub fn stray_ease(&self) -> f32 {
        let f = self.stray_fade;
        f * f * (3.0 - 2.0 * f)
    }

    /// The napping cat's own little window, away from the notch.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn draw_stray(&self, p: &Painter) {
        self.mascot.draw_loaf(p, STRAY_W * 0.38, STRAY_H * 0.55, 1.0, Instant::now());
    }

    fn draw_cat(&self, p: &Painter, cx: f32, cy: f32, s: f32) {
        if self.cat {
            self.mascot.draw(p, cx, cy, s, Instant::now());
        }
    }

    fn draw_pill(&mut self, p: &Painter, x: f32, w: f32) {
        let (label, color) = if self.pomo.running() {
            (mmss(self.pomo.remaining(Instant::now())), FOCUS)
        } else {
            (hhmm(clock().now), TEXT)
        };
        let (ta, dy) = self.toast_fade();
        let fa = |c: Color| Color { a: c.a * ta, ..c };
        if let Some(t) = self.toast.as_ref().filter(|t| t.song) {
            // Cat, "title | artist" centred between, clock. Short of room, the title keeps the most.
            let side = 46f32.max(18.0 + p.measure(&label, Font::Body) + 10.0);
            let room = w - 2.0 * side;
            let sep = " | ";
            let (tw, sw, aw) = (p.measure(&t.title, Font::Body), p.measure(sep, Font::Body), p.measure(&t.detail, Font::Body));
            let aw = if t.detail.is_empty() { 0.0 } else { aw.min((room - sw - tw).max((room - sw) * 0.4)).max(0.0) };
            let sw = if aw > 0.0 { sw } else { 0.0 };
            let tw = tw.min(room - sw - aw);
            let sx = (CANVAS_W - tw - sw - aw) / 2.0;
            p.text(&t.title, Rect::new(sx, dy, tw + 1.0, PILL_H), Font::Body, fa(TEXT), Align::Left);
            if aw > 0.0 {
                p.text(sep, Rect::new(sx + tw, dy, sw + 1.0, PILL_H), Font::Body, fa(MUTED), Align::Left);
                p.text(&t.detail, Rect::new(sx + tw + sw, dy, aw + 1.0, PILL_H), Font::Body, fa(MUTED), Align::Left);
            }
            p.text(&label, Rect::new(x + 16.0, 0.0, w - 34.0, PILL_H), Font::Body, color, Align::Right);
            return;
        }
        if let Some(t) = &self.toast {
            // A copied colour's toast is titled with its hex code; its dot grows into a swatch.
            // The dot pops in a beat ahead of the text.
            let r = if t.title.starts_with('#') { 6.0 } else { 4.0 } * ta.sqrt();
            p.ellipse(x + 46.0, PILL_H / 2.0, r, r, fa(t.color));
            p.text(&t.title, Rect::new(x + 56.0, dy, 130.0, PILL_H), Font::Body, fa(TEXT), Align::Left);
            // Laid out at the full toast width and pinned to the right edge, so the growing pill
            // reveals the detail instead of squeezing it into an ellipsis.
            // Clipped to the space right of the title, so while the pill is narrow it never overlaps.
            let dw = self.pill_width() - 206.0;
            if w > 206.0 {
                p.push_clip(Rect::new(x + 190.0, 0.0, w - 206.0, PILL_H), 0.0, 1.0);
                p.text(&t.detail, Rect::new(x + w - 16.0 - dw, dy, dw, PILL_H), Font::Small, fa(MUTED), Align::Right);
                p.pop_clip();
            }
            return;
        }
        let h = self.pill_size.1;
        self.fit_pill_lyric(p);
        self.draw_pill_lyric(p, h);
        let align = if self.compact { Align::Center } else { Align::Right };
        p.text(&label, Rect::new(x + 16.0, 0.0, w - 34.0, h), Font::Body, color, align);
    }

    /// Headset mic glow while recording: camera wins when both are live.
    fn capture_color(&self) -> Option<Color> {
        let c = self.capture.as_ref()?;
        if c.cam.is_some() { Some(CAM) } else { c.mic.as_ref().map(|_| MIC) }
    }

    /// The sung line rolls over like a karaoke screen: the old one lifts away quickly, the new one rises in.
    /// Laid out at the lyric pill's final size and centred on the notch, so the growing shape reveals it.
    /// Sizes the lyric pill once per song so every line fits without the pill jumping line to line.
    fn fit_pill_lyric(&mut self, p: &Painter) {
        let Some(media) = &self.media else { return };
        let Ok(snap) = media.shared.lock() else { return };
        let (Some(track), LyricState::Found(lines)) = (&snap.track, &snap.lyrics) else { return };
        let key = format!("{}\u{1}{}\u{1}{}", track.title, track.artist, lines.len());
        if self.pill_fit.0 != key {
            let widest = lines.iter().map(|(_, l)| p.measure(l, Font::Body)).fold(0.0, f32::max);
            self.pill_fit = (key, (widest + LYRIC_PILL_PAD + 8.0).clamp(LYRIC_PILL_W, LYRIC_PILL_MAX));
        }
    }

    fn draw_pill_lyric(&self, p: &Painter, h: f32) {
        let (now, before, at) = &self.pill_line;
        let t = (at.elapsed().as_secs_f32() / LINE_SECS).min(1.0);
        let ease = |t: f32| if t >= 1.0 { 1.0 } else { 1.0 - 2f32.powf(-10.0 * t) };
        let rise = if self.reduced_motion { 0.0 } else { 7.0 };
        let w = self.pill_width() - LYRIC_PILL_PAD;
        let r = Rect::new((CANVAS_W - w) / 2.0, 0.0, w, h);
        let line = |s: &str, a: f32, dy: f32| {
            if a <= 0.0 {
            } else if s.is_empty() {
                // With no line to show, a wave of notes fills the space between the cat and the clock.
                const SCALE: f32 = 0.75;
                let (cx, cy) = (CANVAS_W / 2.0, h / 2.0);
                let count = ((w - 16.0) / (NOTE_GAP * SCALE)) as usize;
                p.push_clip(r, h / 2.0, a);
                p.scaled(SCALE, cx, cy, dy, || draw_notes(p, cx, cy + 4.0, count));
                p.pop_clip();
            } else {
                p.scaled(1.0, 0.0, 0.0, dy, || p.text(s, r, Font::Body, Color { a, ..TEXT }, Align::Center));
            }
        };
        let out = ease((t / 0.55).min(1.0));
        line(before, 1.0 - out, -rise * out);
        let e = ease(t);
        line(now, e, rise * (1.0 - e));
    }

    fn draw_open(&mut self, p: &Painter, x: f32) {
        let alert = self.alert.clone();
        if let Some(alert) = alert {
            self.draw_header(p, x);
            return self.draw_alert(p, x, alert);
        }
        // Plain pages stay centred while the panel narrows back from the lyrics width.
        let mx = x + (self.open_w() - OPEN_W) / 2.0;
        if self.tab != Tab::Music || self.page_from.is_some() || !self.has_track || self.wide == 0.0 {
            return self.draw_pages(p, x, mx);
        }
        // Switching lyrics on or off morphs the player into the other layout while the panel resizes.
        let lyr = if self.ly_morph { self.wide } else { 1.0 };
        // The header and page dots only exist without lyrics, so they fade out as the player moves.
        let keep = self.hits.len();
        if lyr < 1.0 {
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

    /// Header, the current page (sliding if it just changed) and the page indicator.
    /// `x` is the panel's left edge, `mx` where a plain-width page starts inside it.
    fn draw_pages(&mut self, p: &Painter, x: f32, mx: f32) {
        // Header and indicator make way for the lyrics view, fading with the panel's width.
        let panel = Rect::new(x, 0.0, self.open_w(), OPEN_H);
        let chrome = (1.0 - self.wide * 2.0).max(0.0);
        p.push_clip(panel, 0.0, chrome);
        self.draw_header(p, mx);
        p.pop_clip();
        match self.page_from {
            Some((from, dir, at)) => {
                // The new page slides in from the arrow's side while the old one leaves the other way,
                // both clipped to the space between the arrows.
                let t = (at.elapsed().as_secs_f32() / PAGE_SECS).min(1.0);
                let ease = 1.0 - (1.0 - t).powi(3);
                let shift = if self.reduced_motion { 0.0 } else { 36.0 * dir };
                // Bottom stops short of the dots (y 128) but clears the shelf zone's edge (~125 at hover scale).
                let lane = Rect::new(mx + 34.0, 22.0, OPEN_W - 68.0, 104.0);
                let out = 1.0 - (t / 0.6).min(1.0);
                if out > 0.0 {
                    let keep = self.hits.len();
                    self.draw_slide(p, x, mx, from, lane, out, -shift * ease);
                    self.hits.truncate(keep);
                }
                self.draw_slide(p, x, mx, self.tab, lane, ease, shift * (1.0 - ease));
            }
            None => self.draw_page(p, mx, self.tab),
        }
        p.push_clip(panel, 0.0, chrome);
        self.draw_dots(p, mx);
        p.pop_clip();
    }

    /// One side of a page slide. With lyrics on, Music slides as the whole lyrics view,
    /// never the plain player, so turning pages doesn't flash the other layout.
    #[allow(clippy::too_many_arguments)]
    fn draw_slide(&mut self, p: &Painter, x: f32, mx: f32, tab: Tab, lane: Rect, alpha: f32, dx: f32) {
        let lyrics = tab == Tab::Music && self.show_lyrics && self.has_track;
        p.push_clip(if lyrics { Rect::new(x, 0.0, self.open_w(), OPEN_H) } else { lane }, 0.0, alpha);
        p.offset(dx);
        if lyrics {
            self.draw_lyrics(p, x, 1.0);
        } else {
            self.draw_page(p, mx, tab);
        }
        p.offset(0.0);
        p.pop_clip();
    }

    /// Page indicator: a bright capsule gliding along a faint track, one slot per page.
    fn draw_dots(&mut self, p: &Painter, x: f32) {
        const SLOT: f32 = 12.0;
        let n = PAGES.len() as f32;
        let x0 = x + (OPEN_W - n * SLOT) / 2.0;
        p.round(Rect::new(x0 - 3.0, 128.0, n * SLOT + 6.0, 4.0), 2.0, Color { a: 0.45, ..DOT });
        // Follows the page slide's easing, so the capsule lands as the new page does.
        let to = Self::page_index(self.tab) as f32;
        let at = match self.page_from {
            Some((from, _, at)) if !self.reduced_motion => {
                let t = (at.elapsed().as_secs_f32() / PAGE_SECS).min(1.0);
                lerp(Self::page_index(from) as f32, to, 1.0 - (1.0 - t).powi(3))
            }
            _ => to,
        };
        p.round(Rect::new(x0 + at * SLOT - 3.0, 128.0, SLOT + 6.0, 4.0), 2.0, TEXT);
        for (i, tab) in PAGES.into_iter().enumerate() {
            self.hits.push((Rect::new(x0 + i as f32 * SLOT, 120.0, SLOT, 18.0), Action::Tab(tab)));
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
        // Right side, like the closed pill; the cat takes the left.
        p.text(&time, Rect::new(x + OPEN_W - 16.0 - tw - 1.0, 8.0, tw + 1.0, 20.0), Font::Body, TEXT, Align::Right);
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
        self.draw_volume(p, Rect::new(cx + 72.0, 96.0, 104.0, 24.0), 1.0);
    }

    /// The progress bar, showing where it is being dragged to while scrubbed.
    fn draw_progress(&mut self, p: &Painter, left: f32, right: f32, cy: f32, track: &media::Track) {
        let pos = self.seek_to.unwrap_or_else(|| track.position());
        if let Some(hit) = progress(p, left, right, cy, track, pos) {
            self.hits.push((hit, Action::Slider(Slider::Seek)));
        }
    }

    /// Mute button, a bar for the system volume and a next-device button shown at `output` opacity, filling `r`.
    fn draw_volume(&mut self, p: &Painter, r: Rect, output: f32) {
        let Some((level, muted)) = self.volume.as_ref().map(|v| (v.level(), v.muted())) else { return };
        let silent = muted || level == 0.0;
        let glyph = if silent { "\u{E74F}" } else { "\u{E767}" };
        self.icon_button(p, Rect::new(r.x, r.y, 22.0, r.h), glyph, Font::Icon, Action::Mute, BG, if muted { FOCUS } else { MUTED });
        // The bar shrinks as the button fades in, so neither jumps.
        let ow = 26.0 * output;
        let hit = slider(p, Rect::new(r.x + 28.0, r.y + r.h / 2.0 - 1.5, r.w - 28.0 - ow, 3.0), if muted { 0.0 } else { level });
        self.hits.push((hit, Action::Slider(Slider::Volume)));
        if output > 0.01 {
            let keep = self.hits.len();
            let fg = Color { a: MUTED.a * output, ..MUTED };
            let fill = Color { a: BG.a * output, ..BG };
            self.icon_button(p, Rect::new(r.x + r.w - 22.0, r.y, 22.0, r.h), "\u{E7F5}", Font::Icon, Action::OutputNext, fill, fg);
            // Only clickable once mostly visible.
            if output < 0.5 {
                self.hits.truncate(keep);
            }
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
        // Start rects must match draw_music exactly, or the layout jumps when the morph starts or ends.
        self.draw_art(p, &snap, at(Rect::new(mx + 44.0, 31.0, 48.0, 48.0), Rect::new(x + 22.0, 12.0, 62.0, 62.0)));
        // Fonts and casing differ between the layouts, so the text cross-fades as it moves.
        let title = at(Rect::new(mx + 110.0, 36.0, OPEN_W - 154.0, 20.0), Rect::new(x + 100.0, 37.0, 144.0, 20.0));
        p.text(&track.title, title, Font::Body, fade(TEXT, 1.0 - k), Align::Left);
        p.text(&track.title, title, Font::Title, fade(TEXT, k), Align::Left);
        let artist = at(Rect::new(mx + 110.0, 55.0, OPEN_W - 154.0, 16.0), Rect::new(x + 100.0, 56.0, 144.0, 14.0));
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
        // No room for the output button beside the lyrics; it fades out as the player moves aside.
        self.draw_volume(p, at(Rect::new(cx + 72.0, 96.0, 104.0, 24.0), Rect::new(x + 192.0, 106.0, 50.0, 24.0)), 1.0 - k);
        p.round(Rect::new(x + 252.0, 22.0, 1.0, 102.0), 0.0, rgba(1.0, 1.0, 1.0, 0.12 * k));

        // The list waits until the player has mostly moved aside, then drifts in from the right.
        let list = ((k - 0.4) / 0.6).clamp(0.0, 1.0);
        if list > 0.0 {
            let slide = if self.reduced_motion { 0.0 } else { 28.0 * (1.0 - list).powi(2) };
            p.push_clip(Rect::new(x + 256.0, 0.0, w - 256.0, OPEN_H), 0.0, list);
            // Offsets aren't stacked: skip them when settled so a page slide's offset carries the list too.
            if slide != 0.0 {
                p.offset(slide);
            }
            self.draw_lyric_list(p, x, &snap, &track, list >= 0.5);
            if slide != 0.0 {
                p.offset(0.0);
            }
            p.pop_clip();
        }
    }

    fn draw_lyric_list(&mut self, p: &Painter, x: f32, snap: &media::Snapshot, track: &media::Track, clickable: bool) {
        let (bx, top, bh, lw) = (x + 264.0, 16.0, 120.0, 234.0);
        let lines = match &snap.lyrics {
            LyricState::Found(lines) => lines,
            state => {
                let msg = if *state == LyricState::Loading { "Loading lyrics…" } else { "Lyrics not available" };
                draw_notes(p, bx + lw / 2.0, top + 50.0, 5);
                p.text(msg, Rect::new(bx, top + 80.0, lw, 20.0), Font::Small, MUTED, Align::Center);
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

/// A row of music notes bobbing in a wave, shown while a song has no lyrics.
const NOTE_GAP: f32 = 30.0;

/// A wave of `count` notes centred on (`cx`, `cy`).
fn draw_notes(p: &Painter, cx: f32, cy: f32, count: usize) {
    // Wall-clock phase, so the wave keeps moving across redraws without stored state.
    let t = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() % 100_000) as f32 / 1000.0;
    for i in 0..count {
        let phase = t * 2.4 - i as f32 * 0.8;
        let (x, y) = (cx + (i as f32 - (count as f32 - 1.0) / 2.0) * NOTE_GAP, cy + 5.0 * phase.sin());
        let c = rgba(1.0, 1.0, 1.0, 0.35 + 0.3 * (0.5 + 0.5 * phase.sin()));
        // Head sits left of the stem, stem rises 14 DIPs; even notes are a beamed pair.
        let note = |hx: f32, hy: f32| {
            p.ellipse(hx, hy, 3.6, 2.8, c);
            p.line(hx + 3.0, hy, hx + 3.0, hy - 14.0, c, 1.5);
        };
        if i % 2 == 0 {
            note(x - 5.0, y + 2.0);
            note(x + 5.0, y);
            p.line(x - 2.0, y - 12.0, x + 8.0, y - 14.0, c, 3.0);
        } else {
            note(x, y);
            p.line(x + 3.0, y - 14.0, x + 7.0, y - 9.0, c, 1.5);
        }
    }
}

fn date_label() -> String {
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let t = local_time();
    format!("{} {} {} {}", DAYS[t.weekday as usize % 7], t.day, MONTHS[(t.month as usize + 11) % 12], t.year)
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

/// Shows `alert` now or queues it. Returns true when it is shown. A pomodoro alert outranks
/// whatever is showing; task reminders wait their turn because `Todo::due` fires them only once.
fn place_alert(current: &mut Option<Alert>, queued: &mut VecDeque<Alert>, alert: Alert) -> bool {
    if current.is_none() || matches!(alert, Alert::Pomodoro(_)) {
        if let Some(old @ Alert::Task(_)) = current.replace(alert) {
            queued.push_front(old);
        }
        true
    } else {
        if matches!(alert, Alert::Task(_)) {
            queued.push_back(alert);
        }
        false
    }
}

/// Length of the pill's visible outline pushed `d` outside it, from `top` down the left side,
/// round the bottom and up the right side.
fn outline_len(w: f32, h: f32, r: f32, d: f32, top: f32) -> f32 {
    let side = (h - r - top).max(0.0);
    2.0 * side + std::f32::consts::PI * (r + d) + (w - 2.0 * r).max(0.0)
}

/// The point `s` DIPs along that outline.
fn outline_at(x: f32, w: f32, h: f32, r: f32, d: f32, top: f32, s: f32) -> (f32, f32) {
    use std::f32::consts::{FRAC_PI_2, PI};
    let (cy, big) = (h - r, r + d);
    let side = (cy - top).max(0.0);
    let arc = FRAC_PI_2 * big;
    let flat = (w - 2.0 * r).max(0.0);
    let (lx, rx) = (x + r, x + w - r);
    let mut s = s.max(0.0);
    if s < side {
        return (x - d, top + s);
    }
    s -= side;
    if s < arc {
        let a = PI - s / big;
        return (lx + big * a.cos(), cy + big * a.sin());
    }
    s -= arc;
    if s < flat {
        return (lx + s, cy + big);
    }
    s -= flat;
    if s < arc {
        let a = FRAC_PI_2 - s / big;
        return (rx + big * a.cos(), cy + big * a.sin());
    }
    s -= arc;
    (x + w + d, cy - s.min(side))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outline_runs_down_round_and_back_up() {
        let (x, w, h, r, d, top) = (100.0, 150.0, 30.0, 15.0, 0.0, 5.0);
        let len = outline_len(w, h, r, d, top);
        let near = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0).abs() < 0.01 && (a.1 - b.1).abs() < 0.01;
        assert!(near(outline_at(x, w, h, r, d, top, 0.0), (100.0, 5.0)), "starts top-left");
        assert!(near(outline_at(x, w, h, r, d, top, len / 2.0), (175.0, 30.0)), "halfway is bottom centre");
        assert!(near(outline_at(x, w, h, r, d, top, len), (250.0, 5.0)), "ends top-right");
        // Evenly spaced samples stay evenly spaced: no jumps where the pieces meet.
        let pts: Vec<_> = (0..=100).map(|i| outline_at(x, w, h, r, d, top, len * i as f32 / 100.0)).collect();
        let step = len / 100.0;
        assert!(pts.windows(2).all(|p| ((p[1].0 - p[0].0).hypot(p[1].1 - p[0].1) - step).abs() < 0.05));
    }

    #[test]
    fn task_alerts_wait_instead_of_dropping() {
        let (mut cur, mut q) = (None, VecDeque::new());
        assert!(place_alert(&mut cur, &mut q, Alert::Water(Prompt::Drink)));
        assert!(!place_alert(&mut cur, &mut q, Alert::Task("a".into())));
        assert!(!place_alert(&mut cur, &mut q, Alert::Water(Prompt::Refill)));
        assert_eq!(q, [Alert::Task("a".into())]);

        cur = None;
        let next = q.pop_front().unwrap();
        assert!(place_alert(&mut cur, &mut q, next));
        // A pomodoro bumps the shown task back to the front of the queue.
        assert!(place_alert(&mut cur, &mut q, Alert::Pomodoro(Mode::Short)));
        assert_eq!(q, [Alert::Task("a".into())]);
    }
}
