//! Drawing types shared by the platform renderers: Direct2D on Windows, cairo on Linux.
//! Both draw in DIPs and expose the same `Painter` methods to the app.

#[cfg(windows)]
#[path = "win/ui.rs"]
mod imp;
#[cfg(target_os = "linux")]
#[path = "linux/ui.rs"]
mod imp;

pub use imp::*;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Color {
    Color { r, g, b, a }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }
    /// For the Windows renderer's partial repaints.
    #[cfg(windows)]
    pub fn union(&self, o: Rect) -> Rect {
        let (x, y) = (self.x.min(o.x), self.y.min(o.y));
        Rect::new(x, y, (self.x + self.w).max(o.x + o.w) - x, (self.y + self.h).max(o.y + o.h) - y)
    }
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

#[derive(Clone, Copy)]
pub enum Font {
    Small,
    Body,
    Title,
    Big,
    /// Segoe Fluent Icons (MDL2 Assets on Windows 10); text is a private-use codepoint like U+E768.
    /// Linux has no such font, so its renderer swaps each codepoint for a symbol or a drawn shape.
    Icon,
    IconBig,
    /// Synced lyric lines: bold and allowed to wrap.
    Lyric,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// Average of BGRA pixels weighted toward colourful ones, lifted so dark art still gives a visible glow.
fn accent(px: &[u8]) -> Color {
    let (mut r, mut g, mut b, mut total) = (0.0, 0.0, 0.0, 0.0);
    for p in px.chunks_exact(4) {
        let (pb, pg, pr) = (p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0);
        let chroma = pr.max(pg).max(pb) - pr.min(pg).min(pb);
        let w = chroma * chroma + 0.01;
        (r, g, b, total) = (r + pr * w, g + pg * w, b + pb * w, total + w);
    }
    let (r, g, b) = (r / total, g / total, b / total);
    let lift = 0.85 / r.max(g).max(b).max(0.05);
    rgba((r * lift).min(1.0), (g * lift).min(1.0), (b * lift).min(1.0), 0.95)
}

#[cfg(test)]
mod tests {
    use super::accent;

    #[test]
    fn accent_prefers_colour_over_grey() {
        // Mostly dark grey with a few red pixels: red should win.
        let mut px = [40u8, 40, 40, 255].repeat(60);
        px.extend([0u8, 0, 200, 255].repeat(4));
        let c = accent(&px);
        assert!(c.r > 0.8 && c.g < 0.4 && c.b < 0.4);
        // All black stays defined.
        assert!(!accent(&[0u8, 0, 0, 255].repeat(4)).r.is_nan());
    }
}
