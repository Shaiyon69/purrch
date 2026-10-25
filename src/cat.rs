//! The notch cat: glowing eyes peeking out of the black notch, with ears, nose and whiskers.
//! Its rhythm follows Nusa's mascot: drowsy after 15 s without input, asleep 5.5 s later,
//! and a startled, happy wake-up with a paw wave when you come back.

use crate::ui::{Align, Color, Font, Painter, Rect, rgba};
use std::f32::consts::PI;
use std::time::{Duration, Instant};

const EYE: Color = rgba(1.0, 0.76, 0.28, 1.0);
const PUPIL: Color = rgba(0.06, 0.03, 0.0, 1.0);
const EAR: Color = rgba(0.16, 0.16, 0.17, 1.0);
const EAR_IN: Color = rgba(0.62, 0.32, 0.4, 1.0);
const NOSE: Color = rgba(0.95, 0.5, 0.56, 1.0);
const WHISKER: Color = rgba(0.42, 0.42, 0.45, 0.8);
/// Black fur, like the rest of the cat; a rim and sheen keep it visible on the black notch.
const PAW: Color = rgba(0.02, 0.02, 0.03, 1.0);
const PAW_RIM: Color = rgba(0.3, 0.3, 0.33, 1.0);
const PAW_SHEEN: Color = rgba(0.62, 0.62, 0.66, 0.9);
const BEAN: Color = rgba(0.95, 0.55, 0.62, 1.0);
const BLUSH: Color = rgba(1.0, 0.5, 0.62, 0.5);
const MOUTH: Color = rgba(0.8, 0.42, 0.5, 1.0);
const ZZZ: Color = rgba(0.62, 0.62, 0.66, 1.0);

const IDLE_MS: u32 = 15_000;
const DROWSY_SECS: f32 = 5.5;
const WAKING_SECS: f32 = 5.2;
const STARTLE_SECS: f32 = 0.3;
const SNORE_PERIOD: f32 = 4.0;
const SNORE_SECS: f32 = 1.4;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mood {
    Awake,
    Drowsy,
    Asleep,
    Waking,
}

/// What the cat is easing toward right now.
#[derive(Clone, Copy)]
struct Pose {
    open: f32,
    eye: (f32, f32),
    happy: f32,
    paw: f32,
    dilate: f32,
}

pub struct Cat {
    pub mood: Mood,
    since: Instant,
    blink_at: Option<Instant>,
    next_blink: Instant,
    rng: u32,
    /// Where the cursor is, as an eye offset in DIPs: x in -3.5..3.5, y in -1..2.
    pub look: (f32, f32),
    pose: Pose,
}

impl Cat {
    pub fn new(now: Instant) -> Cat {
        Cat {
            mood: Mood::Awake,
            since: now,
            blink_at: None,
            next_blink: now + Duration::from_secs(3),
            rng: 0x2545_f491,
            look: (0.0, 0.0),
            pose: Pose { open: 1.0, eye: (0.0, 0.0), happy: 0.0, paw: 0.0, dilate: 0.0 },
        }
    }

    fn rand(&mut self) -> f32 {
        self.rng = self.rng.wrapping_mul(0x41c6_4e6d).wrapping_add(0x3039);
        ((self.rng >> 8) & 0xffff) as f32 / 65535.0
    }

    fn age(&self, now: Instant) -> f32 {
        now.saturating_duration_since(self.since).as_secs_f32()
    }

    fn set(&mut self, mood: Mood, now: Instant) {
        self.mood = mood;
        self.since = now;
    }

    /// Called once a second: an awake cat dozes off once the user has been away long enough.
    pub fn idle(&mut self, idle_ms: u32, panel_open: bool, now: Instant) {
        if self.mood == Mood::Awake && idle_ms >= IDLE_MS && !panel_open {
            self.set(Mood::Drowsy, now);
        }
    }

    /// Fresh input: a dozing cat startles awake and waves.
    pub fn poke(&mut self, now: Instant) {
        if matches!(self.mood, Mood::Drowsy | Mood::Asleep) {
            self.set(Mood::Waking, now);
        }
    }

    /// The eyes follow the cursor only while the cat is paying attention.
    pub fn tracking(&self, now: Instant) -> bool {
        self.mood == Mood::Awake || (self.mood == Mood::Waking && self.age(now) < STARTLE_SECS)
    }

    pub fn waving(&self) -> bool {
        self.pose.paw > 0.01
    }

    fn blink_secs(&self) -> f32 {
        if self.mood == Mood::Drowsy { 0.32 } else { 0.15 }
    }

    fn target(&self, now: Instant) -> Pose {
        let t = self.age(now);
        let rest = Pose { open: 1.0, eye: self.look, happy: 0.0, paw: 0.0, dilate: 0.0 };
        match self.mood {
            Mood::Awake => rest,
            Mood::Waking if t < STARTLE_SECS => Pose { open: 1.15, dilate: 1.0, ..rest },
            Mood::Waking => Pose {
                eye: (0.0, 0.0),
                happy: if t < 3.2 { 1.0 } else { 0.0 },
                paw: if t > 0.4 && t < 2.9 { 1.0 } else { 0.0 },
                ..rest
            },
            // Breathing, with the gaze drifting side to side.
            Mood::Drowsy => Pose { open: 0.9 + 0.05 * (t * 2.4).sin(), eye: (3.5 * (t * 0.9).sin(), 0.6), ..rest },
            Mood::Asleep => Pose { open: 0.12, eye: (0.0, 1.2), ..rest },
        }
    }

    /// Advances moods, blinks and the eased pose. Returns true while anything still moves.
    pub fn step(&mut self, now: Instant, dt: f32) -> bool {
        let t = self.age(now);
        match self.mood {
            Mood::Drowsy if t >= DROWSY_SECS => self.set(Mood::Asleep, now),
            Mood::Waking if t >= WAKING_SECS => self.set(Mood::Awake, now),
            _ => {}
        }
        if let Some(at) = self.blink_at {
            if now.saturating_duration_since(at).as_secs_f32() >= self.blink_secs() {
                self.blink_at = None;
                let (min, range) = if self.mood == Mood::Drowsy { (1.4, 1.2) } else { (1.8, 2.5) };
                let wait = min + self.rand() * range;
                self.next_blink = now + Duration::from_secs_f32(wait);
            }
        } else if self.mood != Mood::Asleep && now >= self.next_blink {
            self.blink_at = Some(now);
        }

        let to = self.target(now);
        let k = 1.0 - (-dt * 14.0).exp();
        let ease = |a: &mut f32, b: f32| *a += (b - *a) * k;
        let p = &mut self.pose;
        ease(&mut p.open, to.open);
        ease(&mut p.eye.0, to.eye.0);
        ease(&mut p.eye.1, to.eye.1);
        ease(&mut p.happy, to.happy);
        ease(&mut p.paw, to.paw);
        ease(&mut p.dilate, to.dilate);
        self.busy(now)
    }

    /// True while frames are needed. The eye timer polls this to wake the render loop
    /// for blinks and snores, so an idle cat costs nothing between them.
    pub fn busy(&self, now: Instant) -> bool {
        let to = self.target(now);
        let p = &self.pose;
        let off = |a: f32, b: f32| (a - b).abs() > 0.01;
        let unsettled = off(p.open, to.open)
            || off(p.eye.0, to.eye.0)
            || off(p.eye.1, to.eye.1)
            || off(p.happy, to.happy)
            || off(p.paw, to.paw)
            || off(p.dilate, to.dilate);
        unsettled
            || self.blink_at.is_some()
            || (self.mood != Mood::Asleep && now >= self.next_blink)
            || matches!(self.mood, Mood::Drowsy | Mood::Waking)
            || (self.mood == Mood::Asleep && self.age(now) % SNORE_PERIOD < SNORE_SECS)
    }

    /// Draws the cat centred on (cx, cy); `s` scales it (1.0 is about 31 DIP wide).
    pub fn draw(&self, p: &Painter, cx: f32, cy: f32, s: f32, now: Instant) {
        let Pose { open, eye: (lx, ly), happy, paw, dilate } = self.pose;
        let blink = self.blink_at.map_or(1.0, |at| {
            let b = (now.saturating_duration_since(at).as_secs_f32() / self.blink_secs()).min(1.0);
            1.0 - 0.9 * (b * PI).sin()
        });

        // Ears perk up when startled and fold down in sleep; they lean a little with the gaze.
        let lift = ((open - 1.0) * 10.0).clamp(-2.0, 1.5) * s;
        let lean = lx * 0.3 * s;
        for side in [-1.0f32, 1.0] {
            let x = |d: f32| cx + side * d * s + lean;
            p.triangle([(x(3.5), cy - 7.0 * s), (x(15.0), cy - 3.5 * s), (x(12.0), cy - 13.0 * s - lift)], EAR);
            p.triangle([(x(6.5), cy - 7.0 * s), (x(13.0), cy - 5.0 * s), (x(11.5), cy - 11.0 * s - lift)], EAR_IN);
            // A rounded tip instead of a sharp point.
            p.ellipse(x(12.0), cy - 12.6 * s - lift, 0.9 * s, 0.9 * s, EAR);
        }

        let glow = open.min(1.0) * 0.65 + 0.35;
        let seen = 1.0 - happy;
        for side in [-1.0f32, 1.0] {
            // Set low and a little apart, kitten-style.
            let (ex, ey) = (cx + (side * 6.5 + lx * 0.8) * s, cy + (0.8 + ly) * s);
            // Big, round kitten eyes.
            let w = 2.9 * s;
            let h = (open * 3.3 * blink).max(0.7) * s;
            if seen > 0.01 {
                p.ellipse(ex, ey, w * 2.0, 1.4 * s + h * 1.4, alpha(EYE, 0.10 * glow * seen));
                p.ellipse(ex, ey, w * 1.45, 0.7 * s + h * 1.2, alpha(EYE, 0.16 * glow * seen));
                if open < 0.3 {
                    // Asleep: a contented closed curve.
                    p.arc(ex, ey - 1.2 * s, 2.4 * s, 25.0, 130.0, alpha(EYE, seen), 1.1 * s);
                } else {
                    p.ellipse(ex, ey, w, h, alpha(EYE, seen));
                    if h > 1.4 * s {
                        let px = ex + lx * 0.3 * s;
                        let pw = (1.9 + 0.5 * dilate) * s;
                        p.ellipse(px, ey + 0.2 * s, pw, (h * 0.78).min(pw * 1.25), alpha(PUPIL, seen));
                        let shine = rgba(1.0, 1.0, 1.0, 0.95 * seen);
                        p.ellipse(px - 0.8 * s, ey - 0.9 * s, 1.0 * s, 1.0 * s, shine);
                        p.ellipse(px + 0.8 * s, ey + 0.8 * s, 0.45 * s, 0.45 * s, shine);
                    }
                }
            }
            if happy > 0.01 {
                p.arc(ex, ey + 1.6 * s, 2.4 * s, 205.0, 130.0, alpha(EYE, happy), 1.3 * s);
            }
        }

        let (nx, ny) = (cx + lx * 0.5 * s, cy + (4.4 + ly * 0.5) * s);
        for side in [-1.0f32, 1.0] {
            // Just below and outside the eyes, so they stay clear at notch size.
            p.ellipse(cx + side * 9.6 * s + lx * 0.6 * s, cy + (5.4 + ly * 0.5) * s, 2.1 * s, 1.2 * s, BLUSH);
        }
        // A soft rounded nose rather than a sharp wedge.
        p.ellipse(nx, ny + 0.35 * s, 1.15 * s, 0.75 * s, NOSE);
        // The little ω mouth.
        for side in [-1.0f32, 1.0] {
            p.arc(nx + side * 0.95 * s, ny + 1.1 * s, 0.95 * s, 10.0, 160.0, MOUTH, 0.5 * s);
        }
        for side in [-1.0f32, 1.0] {
            let x0 = nx + side * 4.0 * s;
            p.line(x0, ny + 0.6 * s, x0 + side * 6.5 * s, ny - 0.2 * s, WHISKER, 0.5 * s);
            p.line(x0, ny + 1.5 * s, x0 + side * 6.5 * s, ny + 2.2 * s, WHISKER, 0.5 * s);
        }

        let t = self.age(now);
        if self.mood == Mood::Asleep {
            let phase = t % SNORE_PERIOD;
            if phase < SNORE_SECS {
                let f = phase / SNORE_SECS;
                let z = Rect::new(cx + (f * 6.0 + 14.0) * s - 4.0, cy - 8.0 * f * s - 14.0, 10.0, 10.0);
                p.text("z", z, Font::Small, alpha(ZZZ, 0.7 * (f * PI).sin()), Align::Left);
            }
        }
        if paw > 0.01 {
            // Rises from below the notch edge and waves hello.
            let wave = (t * 14.0).sin();
            let (px, py) = (cx + 22.0 * s + wave * 1.5 * s, cy + 5.0 * s + (1.0 - paw) * 15.0 * s + wave.abs() * 0.5 * s);
            // A round black paw, pads facing us, outlined by a soft rim and a sheen on its upper left.
            p.ellipse(px, py, 4.8 * s, 4.3 * s, PAW);
            p.arc(px, py, 4.5 * s, 0.0, 360.0, PAW_RIM, 0.6 * s);
            p.arc(px, py, 3.9 * s, 200.0, 70.0, PAW_SHEEN, 0.7 * s);
            // Four toe beans in an arc...
            for (dx, dy) in [(-3.0f32, -2.4f32), (-1.05, -3.4), (1.05, -3.4), (3.0, -2.4)] {
                p.ellipse(px + dx * s, py + dy * s, 0.95 * s, 1.1 * s, BEAN);
            }
            // ...and the three-lobed main pad.
            p.ellipse(px - 1.15 * s, py + 0.7 * s, 1.25 * s, 1.15 * s, BEAN);
            p.ellipse(px + 1.15 * s, py + 0.7 * s, 1.25 * s, 1.15 * s, BEAN);
            p.ellipse(px, py + 1.4 * s, 1.75 * s, 1.25 * s, BEAN);
        }
    }
}

fn alpha(c: Color, a: f32) -> Color {
    rgba(c.r, c.g, c.b, c.a * a.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(start: Instant, secs: f32) -> Instant {
        start + Duration::from_secs_f32(secs)
    }

    #[test]
    fn dozes_sleeps_and_wakes_on_nusas_schedule() {
        let t0 = Instant::now();
        let mut cat = Cat::new(t0);
        cat.idle(14_000, false, t0);
        assert_eq!(cat.mood, Mood::Awake);
        cat.idle(15_000, true, t0);
        assert_eq!(cat.mood, Mood::Awake, "an open panel keeps it awake");
        cat.idle(15_000, false, t0);
        assert_eq!(cat.mood, Mood::Drowsy);
        cat.step(at(t0, 5.6), 0.016);
        assert_eq!(cat.mood, Mood::Asleep);
        let woke = at(t0, 7.0);
        cat.poke(woke);
        assert_eq!(cat.mood, Mood::Waking);
        assert!(cat.tracking(woke) && !cat.tracking(at(t0, 7.5)));
        cat.step(at(t0, 12.3), 0.016);
        assert_eq!(cat.mood, Mood::Awake);
        cat.poke(at(t0, 13.0));
        assert_eq!(cat.mood, Mood::Awake, "poking an awake cat changes nothing");
    }

    #[test]
    fn settles_and_goes_quiet_between_blinks() {
        let t0 = Instant::now();
        let mut cat = Cat::new(t0);
        assert!(!cat.busy(at(t0, 1.0)), "awake and settled before its first blink");
        assert!(cat.busy(at(t0, 3.0)), "first blink is due");
    }
}
