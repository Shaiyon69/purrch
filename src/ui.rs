//! Per-pixel-alpha drawing: Direct2D paints into a DIB, UpdateLayeredWindow shows it.
//! Fully transparent pixels let clicks through, so the notch only catches the mouse where it is drawn.

use std::cell::Cell;
use std::ffi::c_void;
use std::mem::ManuallyDrop;
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::{BOOL, COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::UI::Shell::{IShellItemImageFactory, SHCreateItemFromParsingName, SHCreateMemStream, SIIGBF_RESIZETOFIT};
use windows::Win32::UI::WindowsAndMessaging::{UPDATELAYEREDWINDOWINFO, ULW_ALPHA, UpdateLayeredWindowIndirect};
use windows::core::{HSTRING, Interface, Result, w};

pub type Color = D2D1_COLOR_F;

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
    pub fn union(&self, o: Rect) -> Rect {
        let (x, y) = (self.x.min(o.x), self.y.min(o.y));
        Rect::new(x, y, (self.x + self.w).max(o.x + o.w) - x, (self.y + self.h).max(o.y + o.h) - y)
    }
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
    fn d2d(&self) -> D2D_RECT_F {
        D2D_RECT_F { left: self.x, top: self.y, right: self.x + self.w, bottom: self.y + self.h }
    }
}

#[derive(Clone, Copy)]
pub enum Font {
    Small,
    Body,
    Title,
    Big,
    /// Segoe Fluent Icons (MDL2 Assets on Windows 10); text is a private-use codepoint like U+E768.
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

pub struct Renderer {
    factory: ID2D1Factory,
    target: ID2D1DCRenderTarget,
    brush: ID2D1SolidColorBrush,
    /// One per nesting level of `push_clip`: morph fade, page slide, layout cross-fade, card fade, thumbnail.
    layers: [ID2D1Layer; 5],
    dashed: ID2D1StrokeStyle,
    round_cap: ID2D1StrokeStyle,
    dw: IDWriteFactory,
    fonts: [IDWriteTextFormat; 7],
    dc: HDC,
    bitmap: HBITMAP,
    /// Canvas size in device pixels.
    pub size: SIZE,
}

impl Renderer {
    pub fn new(width: i32, height: i32) -> Result<Renderer> {
        unsafe {
            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                ..Default::default()
            };
            let target = factory.CreateDCRenderTarget(&props)?;
            target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let brush = target.CreateSolidColorBrush(&rgba(1.0, 1.0, 1.0, 1.0), None)?;
            let layer = || target.CreateLayer(None);
            let layers = [layer()?, layer()?, layer()?, layer()?, layer()?];
            let stroke = |dash: D2D1_DASH_STYLE, cap: D2D1_CAP_STYLE| {
                let props = D2D1_STROKE_STYLE_PROPERTIES {
                    startCap: cap,
                    endCap: cap,
                    dashCap: cap,
                    lineJoin: D2D1_LINE_JOIN_ROUND,
                    miterLimit: 10.0,
                    dashStyle: dash,
                    dashOffset: 0.0,
                };
                factory.CreateStrokeStyle(&props, None)
            };
            let dashed = stroke(D2D1_DASH_STYLE_DASH, D2D1_CAP_STYLE_ROUND)?;
            let round_cap = stroke(D2D1_DASH_STYLE_SOLID, D2D1_CAP_STYLE_ROUND)?;

            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let mut fonts_installed = None;
            dw.GetSystemFontCollection(&mut fonts_installed, false)?;
            let installed = |name| {
                let (mut i, mut exists) = (0, BOOL(0));
                fonts_installed.as_ref().is_some_and(|c| c.FindFamilyName(name, &mut i, &mut exists).is_ok() && exists.as_bool())
            };
            let icons = if installed(w!("Segoe Fluent Icons")) { w!("Segoe Fluent Icons") } else { w!("Segoe MDL2 Assets") };
            let font = |family, size: f32, weight: DWRITE_FONT_WEIGHT| -> Result<IDWriteTextFormat> {
                let f = dw.CreateTextFormat(
                    family,
                    None,
                    weight,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    w!("en-us"),
                )?;
                f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
                f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
                // Long titles end in an ellipsis instead of being cut mid-glyph.
                let trim = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
                f.SetTrimming(&trim, &dw.CreateEllipsisTrimmingSign(&f)?)?;
                Ok(f)
            };
            let ui = w!("Segoe UI");
            let fonts = [
                font(ui, 11.0, DWRITE_FONT_WEIGHT_NORMAL)?,
                font(ui, 13.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                font(ui, 15.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                font(ui, 28.0, DWRITE_FONT_WEIGHT_BOLD)?,
                font(icons, 13.0, DWRITE_FONT_WEIGHT_NORMAL)?,
                font(icons, 18.0, DWRITE_FONT_WEIGHT_NORMAL)?,
                font(ui, 16.0, DWRITE_FONT_WEIGHT_BOLD)?,
            ];
            fonts[Font::Lyric as usize].SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;

            let dc = CreateCompatibleDC(None);
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height, // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
            SelectObject(dc, bitmap);
            target.BindDC(dc, &RECT { left: 0, top: 0, right: width, bottom: height })?;

            Ok(Renderer { factory, target, brush, layers, dashed, round_cap, dw, fonts, dc, bitmap, size: SIZE { cx: width, cy: height } })
        }
    }

    /// Draw a frame with `paint` and push it to the window. With `dirty` (DIPs) only that
    /// area is cleared, redrawn and uploaded; the rest keeps the previous frame.
    pub fn frame(&self, hwnd: HWND, dirty: Option<Rect>, paint: impl FnOnce(&Painter)) -> Result<()> {
        unsafe {
            let (mut dpi, mut _dpi_y) = (96.0, 96.0);
            self.target.GetDpi(&mut dpi, &mut _dpi_y);
            let s = dpi / 96.0;
            let px = dirty.map_or(RECT { left: 0, top: 0, right: self.size.cx, bottom: self.size.cy }, |d| RECT {
                left: ((d.x * s).floor() as i32 - 1).max(0),
                top: ((d.y * s).floor() as i32 - 1).max(0),
                right: (((d.x + d.w) * s).ceil() as i32 + 1).min(self.size.cx),
                bottom: (((d.y + d.h) * s).ceil() as i32 + 1).min(self.size.cy),
            });
            let clip = D2D_RECT_F {
                left: px.left as f32 / s,
                top: px.top as f32 / s,
                right: px.right as f32 / s,
                bottom: px.bottom as f32 / s,
            };
            self.target.BeginDraw();
            self.target.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_ALIASED);
            self.target.Clear(Some(&rgba(0.0, 0.0, 0.0, 0.0)));
            paint(&Painter { r: self, depth: Cell::new(0) });
            self.target.PopAxisAlignedClip();
            self.target.EndDraw(None, None)?;
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
                ..Default::default()
            };
            let origin = POINT::default();
            let info = UPDATELAYEREDWINDOWINFO {
                cbSize: size_of::<UPDATELAYEREDWINDOWINFO>() as u32,
                hdcDst: HDC::default(),
                pptDst: std::ptr::null(),
                psize: &self.size,
                hdcSrc: self.dc,
                pptSrc: &origin,
                crKey: COLORREF(0),
                pblend: &blend,
                dwFlags: ULW_ALPHA,
                prcDirty: &px,
            };
            UpdateLayeredWindowIndirect(hwnd, &info).ok()
        }
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.bitmap);
            let _ = DeleteDC(self.dc);
        }
    }
}

/// Immediate-mode drawing helpers, all in DIPs.
pub struct Painter<'a> {
    r: &'a Renderer,
    depth: Cell<usize>,
}

impl Painter<'_> {
    fn brush(&self, c: Color) -> &ID2D1SolidColorBrush {
        unsafe { self.r.brush.SetColor(&c) };
        &self.r.brush
    }

    /// Decodes PNG/JPEG bytes into a bitmap for this render target.
    /// Decodes an image and picks its accent colour from a 16x16 downscale.
    pub fn load_image(&self, bytes: &[u8]) -> Option<(ID2D1Bitmap, Color)> {
        unsafe {
            let stream = SHCreateMemStream(Some(bytes))?;
            let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
            let decoder = wic.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand).ok()?;
            let frame = decoder.GetFrame(0).ok()?;
            let converter = wic.CreateFormatConverter().ok()?;
            converter
                .Initialize(&frame, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut)
                .ok()?;
            let scaler = wic.CreateBitmapScaler().ok()?;
            scaler.Initialize(&converter, 16, 16, WICBitmapInterpolationModeFant).ok()?;
            let mut px = [0u8; 16 * 16 * 4];
            scaler.CopyPixels(std::ptr::null(), 16 * 4, &mut px).ok()?;
            Some((self.r.target.CreateBitmapFromWicBitmap(&converter, None).ok()?, accent(&px)))
        }
    }

    /// The shell's thumbnail for a file (photo preview, or its icon), `px` pixels square at most.
    pub fn load_thumb(&self, path: &std::path::Path, px: i32) -> Option<ID2D1Bitmap> {
        unsafe {
            let item: IShellItemImageFactory = SHCreateItemFromParsingName(&HSTRING::from(path.as_os_str()), None).ok()?;
            let hbmp = item.GetImage(SIZE { cx: px, cy: px }, SIIGBF_RESIZETOFIT).ok()?;
            let mut bm = BITMAP::default();
            GetObjectW(hbmp, size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut c_void));
            let (w, h) = (bm.bmWidth, bm.bmHeight.abs());
            let mut info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = vec![0u8; (w * h * 4).max(0) as usize];
            let rows = GetDIBits(self.r.dc, hbmp, 0, h as u32, Some(bits.as_mut_ptr() as *mut c_void), &mut info, DIB_RGB_COLORS);
            let _ = DeleteObject(hbmp);
            if rows == 0 || w == 0 {
                return None;
            }
            // Photo previews come back with every alpha byte zero; those are opaque.
            if bits.chunks(4).all(|p| p[3] == 0) {
                bits.chunks_mut(4).for_each(|p| p[3] = 255);
            }
            let props = D2D1_BITMAP_PROPERTIES {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
            };
            let size = D2D_SIZE_U { width: w as u32, height: h as u32 };
            self.r.target.CreateBitmap(size, Some(bits.as_ptr() as *const c_void), w as u32 * 4, &props).ok()
        }
    }

    pub fn image(&self, bitmap: &ID2D1Bitmap, r: Rect) {
        unsafe { self.r.target.DrawBitmap(bitmap, Some(&r.d2d()), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None) };
    }

    pub fn round(&self, r: Rect, radius: f32, c: Color) {
        let rr = D2D1_ROUNDED_RECT { rect: r.d2d(), radiusX: radius, radiusY: radius };
        unsafe { self.r.target.FillRoundedRectangle(&rr, self.brush(c)) };
    }


    /// Clips what follows to a rounded rect and fades it by `opacity`. Pair with `pop_clip`.
    pub fn push_clip(&self, r: Rect, radius: f32, opacity: f32) {
        let rr = D2D1_ROUNDED_RECT { rect: r.d2d(), radiusX: radius, radiusY: radius };
        unsafe {
            let mask = self.r.factory.CreateRoundedRectangleGeometry(&rr).ok().and_then(|g| g.cast::<ID2D1Geometry>().ok());
            let params = D2D1_LAYER_PARAMETERS {
                contentBounds: r.d2d(),
                geometricMask: ManuallyDrop::new(mask),
                maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                maskTransform: Matrix3x2::identity(),
                opacity,
                opacityBrush: ManuallyDrop::new(None),
                layerOptions: D2D1_LAYER_OPTIONS_NONE,
            };
            let depth = self.depth.get().min(self.r.layers.len() - 1);
            self.depth.set(depth + 1);
            self.r.target.PushLayer(&params, &self.r.layers[depth]);
            // PushLayer holds its own reference to the mask.
            drop(ManuallyDrop::into_inner(params.geometricMask));
        }
    }

    /// Shifts everything drawn after this horizontally by `dx`; pass 0 to reset.
    pub fn offset(&self, dx: f32) {
        unsafe { self.r.target.SetTransform(&Matrix3x2::translation(dx, 0.0)) };
    }

    /// Runs `f` scaled by `s` around (`ox`, `oy`) and moved down by `dy`, on top of any page offset.
    pub fn scaled(&self, s: f32, ox: f32, oy: f32, dy: f32, f: impl FnOnce()) {
        unsafe {
            let mut old = Matrix3x2::default();
            self.r.target.GetTransform(&mut old);
            let local = Matrix3x2 { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: ox * (1.0 - s), M32: oy * (1.0 - s) + dy };
            self.r.target.SetTransform(&(local * old));
            f();
            self.r.target.SetTransform(&old);
        }
    }

    /// Soft light rising from the bottom of `r`, fading out toward the top.
    pub fn glow(&self, r: Rect, c: Color) {
        let stop = |position: f32, a: f32| D2D1_GRADIENT_STOP { position, color: Color { a: c.a * a, ..c } };
        let stops = [stop(0.0, 1.0), stop(0.3, 0.55), stop(0.7, 0.12), stop(1.0, 0.0)];
        unsafe {
            let Ok(collection) = self.r.target.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP) else { return };
            let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                startPoint: D2D_POINT_2F { x: r.x, y: r.y + r.h },
                endPoint: D2D_POINT_2F { x: r.x, y: r.y },
            };
            let Ok(brush) = self.r.target.CreateLinearGradientBrush(&props, None, &collection) else { return };
            self.r.target.FillRectangle(&r.d2d(), &brush);
        }
    }

    pub fn pop_clip(&self) {
        self.depth.set(self.depth.get().saturating_sub(1));
        unsafe { self.r.target.PopLayer() };
    }

    /// Concave fillets either side of a notch hanging from the top edge (spanning `x..x+w`),
    /// so it reads as part of the bezel rather than a floating capsule.
    pub fn shoulders(&self, x: f32, w: f32, r: f32, c: Color) {
        let p = |x: f32, y: f32| D2D_POINT_2F { x, y };
        unsafe {
            let Ok(geo) = self.r.factory.CreatePathGeometry() else { return };
            let Ok(sink) = geo.Open() else { return };
            for (edge, dir) in [(x, -1.0f32), (x + w, 1.0)] {
                // Tuck half a DIP under the body so antialiased edges don't leave a seam.
                let inner = edge - dir * 0.5;
                sink.BeginFigure(p(edge + dir * r, 0.0), D2D1_FIGURE_BEGIN_FILLED);
                sink.AddLine(p(inner, 0.0));
                sink.AddLine(p(inner, r));
                sink.AddLine(p(edge, r));
                sink.AddArc(&D2D1_ARC_SEGMENT {
                    point: p(edge + dir * r, 0.0),
                    size: D2D_SIZE_F { width: r, height: r },
                    rotationAngle: 0.0,
                    sweepDirection: if dir < 0.0 { D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE } else { D2D1_SWEEP_DIRECTION_CLOCKWISE },
                    arcSize: D2D1_ARC_SIZE_SMALL,
                });
                sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            }
            if sink.Close().is_ok() {
                self.r.target.FillGeometry(&geo, self.brush(c), None);
            }
        }
    }

    /// Outline of a rounded rect, optionally dashed.
    pub fn stroke_round(&self, r: Rect, radius: f32, c: Color, width: f32, dashed: bool) {
        let rr = D2D1_ROUNDED_RECT { rect: r.d2d(), radiusX: radius, radiusY: radius };
        let style = dashed.then_some(&self.r.dashed);
        unsafe { self.r.target.DrawRoundedRectangle(&rr, self.brush(c), width, style) };
    }

    /// Stroked arc with round caps. Angles in degrees, clockwise from 3 o'clock.
    pub fn arc(&self, cx: f32, cy: f32, radius: f32, from: f32, sweep: f32, c: Color, width: f32) {
        if sweep <= 0.0 {
            return;
        }
        let at = |deg: f32| {
            let a = deg.to_radians();
            D2D_POINT_2F { x: cx + radius * a.cos(), y: cy + radius * a.sin() }
        };
        unsafe {
            if sweep >= 359.9 {
                let e = D2D1_ELLIPSE { point: D2D_POINT_2F { x: cx, y: cy }, radiusX: radius, radiusY: radius };
                self.r.target.DrawEllipse(&e, self.brush(c), width, None);
                return;
            }
            let Ok(geo) = self.r.factory.CreatePathGeometry() else { return };
            let Ok(sink) = geo.Open() else { return };
            sink.BeginFigure(at(from), D2D1_FIGURE_BEGIN_HOLLOW);
            sink.AddArc(&D2D1_ARC_SEGMENT {
                point: at(from + sweep),
                size: D2D_SIZE_F { width: radius, height: radius },
                rotationAngle: 0.0,
                sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                arcSize: if sweep > 180.0 { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
            });
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
            if sink.Close().is_ok() {
                self.r.target.DrawGeometry(&geo, self.brush(c), width, &self.r.round_cap);
            }
        }
    }

    /// Width of `s` in DIPs when drawn in `font`.
    pub fn measure(&self, s: &str, font: Font) -> f32 {
        let wide: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            let Ok(layout) = self.r.dw.CreateTextLayout(&wide, &self.r.fonts[font as usize], 4096.0, 64.0) else { return 0.0 };
            let mut m = DWRITE_TEXT_METRICS::default();
            if layout.GetMetrics(&mut m).is_err() {
                return 0.0;
            }
            m.widthIncludingTrailingWhitespace
        }
    }

    /// Height of `s` wrapped to `width`, for fonts that wrap.
    pub fn measure_height(&self, s: &str, font: Font, width: f32) -> f32 {
        let wide: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            let Ok(layout) = self.r.dw.CreateTextLayout(&wide, &self.r.fonts[font as usize], width, 4096.0) else { return 0.0 };
            let mut m = DWRITE_TEXT_METRICS::default();
            if layout.GetMetrics(&mut m).is_err() {
                return 0.0;
            }
            m.height
        }
    }

    pub fn ellipse(&self, x: f32, y: f32, rx: f32, ry: f32, c: Color) {
        let e = D2D1_ELLIPSE { point: D2D_POINT_2F { x, y }, radiusX: rx, radiusY: ry };
        unsafe { self.r.target.FillEllipse(&e, self.brush(c)) };
    }

    pub fn line(&self, x0: f32, y0: f32, x1: f32, y1: f32, c: Color, width: f32) {
        unsafe {
            self.r.target.DrawLine(
                D2D_POINT_2F { x: x0, y: y0 },
                D2D_POINT_2F { x: x1, y: y1 },
                self.brush(c),
                width,
                None,
            )
        };
    }

    pub fn triangle(&self, pts: [(f32, f32); 3], c: Color) {
        let p = |(x, y): (f32, f32)| D2D_POINT_2F { x, y };
        unsafe {
            let Ok(geo) = self.r.factory.CreatePathGeometry() else { return };
            let Ok(sink) = geo.Open() else { return };
            sink.BeginFigure(p(pts[0]), D2D1_FIGURE_BEGIN_FILLED);
            sink.AddLine(p(pts[1]));
            sink.AddLine(p(pts[2]));
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            if sink.Close().is_ok() {
                self.r.target.FillGeometry(&geo, self.brush(c), None);
            }
        }
    }

    pub fn text(&self, s: &str, r: Rect, font: Font, c: Color, align: Align) {
        let format = &self.r.fonts[font as usize];
        let wide: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            let _ = format.SetTextAlignment(match align {
                Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
                Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
            });
            self.r.target.DrawText(
                &wide,
                format,
                &r.d2d(),
                self.brush(c),
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }
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
