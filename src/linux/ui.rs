//! Linux drawing: cairo shapes and pango text in a GTK DrawingArea, in DIPs (GTK applies the
//! display scale). Mirrors the Direct2D painter method for method.

use super::{Align, Color, Font, Rect, accent};
use gtk::gdk::prelude::GdkCairoContextExt;
use gtk::gdk_pixbuf::{InterpType, Pixbuf, PixbufLoader};
use gtk::prelude::*;
use gtk::{cairo, pango};
use std::cell::RefCell;
use std::f64::consts::PI;

pub type Image = Pixbuf;

pub struct Painter<'a> {
    cr: &'a cairo::Context,
    /// The transform when the frame began, so `offset` replaces the page shift instead of stacking it.
    base: cairo::Matrix,
    /// Opacity of each open `push_clip`, applied as it closes.
    layers: RefCell<Vec<f32>>,
}

/// (size in DIPs, weight) per `Font`, matching the Segoe UI sizes on Windows.
fn spec(font: Font) -> (f64, pango::Weight) {
    match font {
        Font::Small => (11.0, pango::Weight::Normal),
        Font::Body => (13.0, pango::Weight::Semibold),
        Font::Title => (15.0, pango::Weight::Semibold),
        Font::Big => (28.0, pango::Weight::Bold),
        Font::Icon => (13.0, pango::Weight::Normal),
        Font::IconBig => (18.0, pango::Weight::Normal),
        Font::Lyric => (16.0, pango::Weight::Bold),
    }
}

/// Segoe Fluent Icons codepoints the app uses, as something every Linux desktop can show.
enum Glyph {
    Text(&'static str),
    Speaker,
    Mute,
    Chart,
    Folder,
    Document,
}

fn glyph(c: char) -> Option<Glyph> {
    Some(match c {
        // VS15 keeps the bolt out of the colour emoji font.
        '\u{E945}' => Glyph::Text("\u{26A1}\u{FE0E}"),
        '\u{E90A}' => Glyph::Text("\u{2261}"),
        '\u{E7F5}' => Glyph::Text("\u{21C4}"),
        '\u{E8D6}' => Glyph::Text("\u{266B}"),
        '\u{E72C}' => Glyph::Text("\u{21BB}"),
        '\u{E713}' => Glyph::Text("\u{2699}\u{FE0E}"),
        '\u{E738}' => Glyph::Text("\u{2212}"),
        '\u{E710}' => Glyph::Text("+"),
        '\u{E73E}' => Glyph::Text("\u{2713}"),
        '\u{E711}' => Glyph::Text("\u{2715}"),
        '\u{E896}' => Glyph::Text("\u{2193}"),
        '\u{E767}' => Glyph::Speaker,
        '\u{E74F}' => Glyph::Mute,
        '\u{E9D2}' => Glyph::Chart,
        '\u{E8B7}' => Glyph::Folder,
        '\u{E8A5}' => Glyph::Document,
        _ => return None,
    })
}

fn is_icon(c: char) -> bool {
    ('\u{E000}'..='\u{F8FF}').contains(&c)
}

impl<'a> Painter<'a> {
    pub fn new(cr: &'a cairo::Context) -> Painter<'a> {
        Painter { cr, base: cr.matrix(), layers: RefCell::new(Vec::new()) }
    }

    fn color(&self, c: Color) {
        self.cr.set_source_rgba(c.r as f64, c.g as f64, c.b as f64, c.a as f64);
    }

    fn rounded(&self, r: Rect, radius: f32) {
        let (x, y, w, h) = (r.x as f64, r.y as f64, r.w as f64, r.h as f64);
        let rad = (radius as f64).min(w / 2.0).min(h / 2.0).max(0.0);
        let cr = self.cr;
        cr.new_sub_path();
        cr.arc(x + w - rad, y + rad, rad, -PI / 2.0, 0.0);
        cr.arc(x + w - rad, y + h - rad, rad, 0.0, PI / 2.0);
        cr.arc(x + rad, y + h - rad, rad, PI / 2.0, PI);
        cr.arc(x + rad, y + rad, rad, PI, 1.5 * PI);
        cr.close_path();
    }

    /// Decodes an image and picks its accent colour from a 16x16 downscale.
    pub fn load_image(&self, bytes: &[u8]) -> Option<(Image, Color)> {
        let loader = PixbufLoader::new();
        loader.write(bytes).ok()?;
        loader.close().ok()?;
        let image = loader.pixbuf()?;
        let small = image.scale_simple(16, 16, InterpType::Bilinear)?;
        let (n, stride, px) = (small.n_channels() as usize, small.rowstride() as usize, small.read_pixel_bytes());
        // accent() reads BGRA like the Windows bitmaps.
        let mut bgra = Vec::with_capacity(16 * 16 * 4);
        for y in 0..16 {
            for x in 0..16 {
                let p = &px[y * stride + x * n..][..n];
                bgra.extend([p[2], p[1], p[0], 255]);
            }
        }
        Some((image, accent(&bgra)))
    }

    /// A preview for image files, `px` pixels square at most. Others get the app's file glyph.
    pub fn load_thumb(&self, path: &std::path::Path, px: i32) -> Option<Image> {
        Pixbuf::from_file_at_scale(path, px, px, true).ok()
    }

    pub fn image(&self, image: &Image, r: Rect) {
        let (w, h) = (image.width().max(1) as f64, image.height().max(1) as f64);
        let cr = self.cr;
        cr.save().ok();
        cr.translate(r.x as f64, r.y as f64);
        cr.scale(r.w as f64 / w, r.h as f64 / h);
        cr.set_source_pixbuf(image, 0.0, 0.0);
        let _ = cr.paint();
        cr.restore().ok();
    }

    pub fn round(&self, r: Rect, radius: f32, c: Color) {
        self.rounded(r, radius);
        self.color(c);
        let _ = self.cr.fill();
    }

    /// Clips what follows to a rounded rect and fades it by `opacity`. Pair with `pop_clip`.
    pub fn push_clip(&self, r: Rect, radius: f32, opacity: f32) {
        self.cr.save().ok();
        self.rounded(r, radius);
        self.cr.clip();
        self.cr.push_group();
        self.layers.borrow_mut().push(opacity);
    }

    pub fn pop_clip(&self) {
        let Some(opacity) = self.layers.borrow_mut().pop() else { return };
        // A transform set inside the layer outlives it, as with Direct2D layers.
        let m = self.cr.matrix();
        let _ = self.cr.pop_group_to_source();
        let _ = self.cr.paint_with_alpha(opacity as f64);
        self.cr.restore().ok();
        self.cr.set_matrix(m);
    }

    /// Shifts everything drawn after this horizontally by `dx`; pass 0 to reset.
    pub fn offset(&self, dx: f32) {
        self.cr.set_matrix(self.base);
        self.cr.translate(dx as f64, 0.0);
    }

    /// Runs `f` scaled by `s` around (`ox`, `oy`) and moved down by `dy`, on top of any page offset.
    pub fn scaled(&self, s: f32, ox: f32, oy: f32, dy: f32, f: impl FnOnce()) {
        let old = self.cr.matrix();
        self.cr.translate((ox * (1.0 - s)) as f64, (oy * (1.0 - s) + dy) as f64);
        self.cr.scale(s as f64, s as f64);
        f();
        self.cr.set_matrix(old);
    }

    /// Soft light rising from the bottom of `r`, fading out toward the top.
    pub fn glow(&self, r: Rect, c: Color) {
        let g = cairo::LinearGradient::new(r.x as f64, (r.y + r.h) as f64, r.x as f64, r.y as f64);
        for (pos, a) in [(0.0, 1.0), (0.3, 0.55), (0.7, 0.12), (1.0, 0.0)] {
            g.add_color_stop_rgba(pos, c.r as f64, c.g as f64, c.b as f64, (c.a * a) as f64);
        }
        self.cr.rectangle(r.x as f64, r.y as f64, r.w as f64, r.h as f64);
        let _ = self.cr.set_source(&g);
        let _ = self.cr.fill();
    }

    /// Concave fillets either side of a notch hanging from the top edge (spanning `x..x+w`),
    /// so it reads as part of the bezel rather than a floating capsule.
    pub fn shoulders(&self, x: f32, w: f32, r: f32, c: Color) {
        let cr = self.cr;
        let r = r as f64;
        for (edge, dir) in [(x as f64, -1.0f64), ((x + w) as f64, 1.0)] {
            // Tuck half a DIP under the body so antialiased edges don't leave a seam.
            let inner = edge - dir * 0.5;
            cr.move_to(edge + dir * r, 0.0);
            cr.line_to(inner, 0.0);
            cr.line_to(inner, r);
            cr.line_to(edge, r);
            if dir < 0.0 {
                cr.arc_negative(edge - r, r, r, 0.0, -PI / 2.0);
            } else {
                cr.arc(edge + r, r, r, PI, 1.5 * PI);
            }
            cr.close_path();
        }
        self.color(c);
        let _ = cr.fill();
    }

    /// Outline of a rounded rect, optionally dashed.
    pub fn stroke_round(&self, r: Rect, radius: f32, c: Color, width: f32, dashed: bool) {
        let cr = self.cr;
        self.rounded(r, radius);
        self.color(c);
        cr.set_line_width(width as f64);
        if dashed {
            // Direct2D's dash style: dash and gap of two stroke widths, with round caps.
            cr.set_dash(&[2.0 * width as f64, 2.0 * width as f64], 0.0);
            cr.set_line_cap(cairo::LineCap::Round);
        }
        let _ = cr.stroke();
        cr.set_dash(&[], 0.0);
        cr.set_line_cap(cairo::LineCap::Butt);
    }

    /// Stroked arc with round caps. Angles in degrees, clockwise from 3 o'clock.
    pub fn arc(&self, cx: f32, cy: f32, radius: f32, from: f32, sweep: f32, c: Color, width: f32) {
        if sweep <= 0.0 {
            return;
        }
        let cr = self.cr;
        cr.new_path();
        let start = (from as f64).to_radians();
        let end = if sweep >= 359.9 { start + 2.0 * PI } else { ((from + sweep) as f64).to_radians() };
        cr.arc(cx as f64, cy as f64, radius as f64, start, end);
        self.color(c);
        cr.set_line_width(width as f64);
        cr.set_line_cap(if sweep >= 359.9 { cairo::LineCap::Butt } else { cairo::LineCap::Round });
        let _ = cr.stroke();
        cr.set_line_cap(cairo::LineCap::Butt);
    }

    fn layout(&self, s: &str, font: Font) -> pango::Layout {
        let layout = pangocairo::functions::create_layout(self.cr);
        let (size, weight) = spec(font);
        let mut desc = pango::FontDescription::from_string("Sans");
        desc.set_absolute_size(size * pango::SCALE as f64);
        desc.set_weight(weight);
        layout.set_font_description(Some(&desc));
        layout.set_text(s);
        layout
    }

    /// Width of `s` in DIPs when drawn in `font`.
    pub fn measure(&self, s: &str, font: Font) -> f32 {
        self.layout(s, font).pixel_extents().1.width() as f32
    }

    /// Height of `s` wrapped to `width`, for fonts that wrap.
    pub fn measure_height(&self, s: &str, font: Font, width: f32) -> f32 {
        let layout = self.layout(s, font);
        layout.set_width((width as f64 * pango::SCALE as f64) as i32);
        layout.set_wrap(pango::WrapMode::WordChar);
        layout.pixel_extents().1.height() as f32
    }

    pub fn ellipse(&self, x: f32, y: f32, rx: f32, ry: f32, c: Color) {
        let cr = self.cr;
        cr.save().ok();
        cr.translate(x as f64, y as f64);
        cr.scale((rx as f64).max(1e-3), (ry as f64).max(1e-3));
        cr.new_path();
        cr.arc(0.0, 0.0, 1.0, 0.0, 2.0 * PI);
        cr.restore().ok();
        self.color(c);
        let _ = cr.fill();
    }

    pub fn line(&self, x0: f32, y0: f32, x1: f32, y1: f32, c: Color, width: f32) {
        let cr = self.cr;
        cr.move_to(x0 as f64, y0 as f64);
        cr.line_to(x1 as f64, y1 as f64);
        self.color(c);
        cr.set_line_width(width as f64);
        let _ = cr.stroke();
    }

    pub fn triangle(&self, pts: [(f32, f32); 3], c: Color) {
        let cr = self.cr;
        cr.move_to(pts[0].0 as f64, pts[0].1 as f64);
        cr.line_to(pts[1].0 as f64, pts[1].1 as f64);
        cr.line_to(pts[2].0 as f64, pts[2].1 as f64);
        cr.close_path();
        self.color(c);
        let _ = cr.fill();
    }

    pub fn text(&self, s: &str, r: Rect, font: Font, c: Color, align: Align) {
        let mut chars = s.chars();
        if let (Some(ch), None) = (chars.next(), chars.next())
            && is_icon(ch)
        {
            return self.icon(ch, r, spec(font).0 as f32, c, align);
        }
        let s: String = s.chars().filter(|c| !is_icon(*c)).collect();
        self.text_plain(&s, r, font, c, align);
    }

    fn text_plain(&self, s: &str, r: Rect, font: Font, c: Color, align: Align) {
        let layout = self.layout(s, font);
        layout.set_width((r.w as f64 * pango::SCALE as f64) as i32);
        if matches!(font, Font::Lyric) {
            layout.set_wrap(pango::WrapMode::WordChar);
        } else {
            // Long titles end in an ellipsis instead of being cut mid-glyph.
            layout.set_ellipsize(pango::EllipsizeMode::End);
        }
        layout.set_alignment(match align {
            Align::Left => pango::Alignment::Left,
            Align::Center => pango::Alignment::Center,
            Align::Right => pango::Alignment::Right,
        });
        let h = layout.pixel_extents().1.height() as f32;
        let cr = self.cr;
        cr.save().ok();
        cr.rectangle(r.x as f64, r.y as f64, r.w as f64, r.h as f64);
        cr.clip();
        // Vertically centred, like DirectWrite's centred paragraphs.
        cr.move_to(r.x as f64, (r.y + (r.h - h) / 2.0) as f64);
        self.color(c);
        pangocairo::functions::show_layout(cr, &layout);
        cr.restore().ok();
    }

    /// Draws one icon codepoint: a stand-in symbol, or a small vector shape `size` DIPs tall.
    fn icon(&self, ch: char, r: Rect, size: f32, c: Color, align: Align) {
        let Some(g) = glyph(ch) else { return };
        let cx = match align {
            Align::Left => r.x + size / 2.0,
            Align::Center => r.x + r.w / 2.0,
            Align::Right => r.x + r.w - size / 2.0,
        } as f64;
        let cy = (r.y + r.h / 2.0) as f64;
        let s = size as f64;
        let cr = self.cr;
        cr.new_path();
        self.color(c);
        cr.set_line_width((s / 11.0).max(1.0));
        cr.set_line_join(cairo::LineJoin::Round);
        let speaker = || {
            cr.move_to(cx - 0.45 * s, cy - 0.16 * s);
            cr.line_to(cx - 0.25 * s, cy - 0.16 * s);
            cr.line_to(cx - 0.02 * s, cy - 0.38 * s);
            cr.line_to(cx - 0.02 * s, cy + 0.38 * s);
            cr.line_to(cx - 0.25 * s, cy + 0.16 * s);
            cr.line_to(cx - 0.45 * s, cy + 0.16 * s);
            cr.close_path();
            let _ = cr.fill();
        };
        match g {
            Glyph::Text(t) => {
                let font = if size >= 18.0 { Font::IconBig } else if size <= 11.0 { Font::Small } else { Font::Icon };
                self.text_plain(t, r, font, c, align);
            }
            Glyph::Speaker => {
                speaker();
                for rad in [0.2, 0.36] {
                    cr.new_sub_path();
                    cr.arc(cx + 0.02 * s, cy, rad * s, -0.8, 0.8);
                }
                let _ = cr.stroke();
            }
            Glyph::Mute => {
                speaker();
                cr.move_to(cx + 0.14 * s, cy - 0.14 * s);
                cr.line_to(cx + 0.42 * s, cy + 0.14 * s);
                cr.move_to(cx + 0.42 * s, cy - 0.14 * s);
                cr.line_to(cx + 0.14 * s, cy + 0.14 * s);
                let _ = cr.stroke();
            }
            Glyph::Chart => {
                for (i, h) in [0.4, 0.65, 0.9].into_iter().enumerate() {
                    let x = cx - 0.4 * s + i as f64 * 0.3 * s;
                    cr.rectangle(x, cy + 0.45 * s - h * s, 0.2 * s, h * s);
                }
                let _ = cr.fill();
            }
            Glyph::Folder => {
                cr.move_to(cx - 0.45 * s, cy + 0.34 * s);
                cr.line_to(cx - 0.45 * s, cy - 0.36 * s);
                cr.line_to(cx - 0.1 * s, cy - 0.36 * s);
                cr.line_to(cx, cy - 0.24 * s);
                cr.line_to(cx + 0.45 * s, cy - 0.24 * s);
                cr.line_to(cx + 0.45 * s, cy + 0.34 * s);
                cr.close_path();
                let _ = cr.stroke();
            }
            Glyph::Document => {
                cr.move_to(cx - 0.32 * s, cy - 0.45 * s);
                cr.line_to(cx + 0.12 * s, cy - 0.45 * s);
                cr.line_to(cx + 0.32 * s, cy - 0.25 * s);
                cr.line_to(cx + 0.32 * s, cy + 0.45 * s);
                cr.line_to(cx - 0.32 * s, cy + 0.45 * s);
                cr.close_path();
                cr.move_to(cx + 0.12 * s, cy - 0.45 * s);
                cr.line_to(cx + 0.12 * s, cy - 0.25 * s);
                cr.line_to(cx + 0.32 * s, cy - 0.25 * s);
                let _ = cr.stroke();
            }
        }
        cr.set_line_join(cairo::LineJoin::Miter);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_icon_the_app_draws_has_a_stand_in() {
        let app = include_str!("../app.rs");
        let mut rest = app;
        while let Some(i) = rest.find("\\u{E") {
            let code = &rest[i + 3..i + 7];
            let ch = char::from_u32(u32::from_str_radix(code, 16).unwrap()).unwrap();
            assert!(super::glyph(ch).is_some(), "no Linux stand-in for U+{code}");
            rest = &rest[i + 7..];
        }
    }
}
