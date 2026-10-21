//! Face renderer for the Wilai HUD overlay. Mirrors the Aurora artifact
//! visual: a central head image inside a cyan HUD ring with animated
//! ticks, cardinals, scan line, waveform bars, and a state label.
//!
//! Rendered with tiny-skia into a raw RGBA buffer the SCTK layer-shell
//! surface uploads via wl_shm every frame.

use std::f32::consts::TAU;
use tiny_skia::{
    BlendMode, Color, FillRule, FilterQuality, Paint, PathBuilder, Pixmap, PixmapPaint,
    PremultipliedColorU8, Stroke, Transform,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaceState {
    Idle,
    Listening,
    Thinking,
    Speaking,
    Acting,
    Alert,
    Offline,
}

impl FaceState {
    pub fn label(&self) -> &'static str {
        match self {
            FaceState::Idle => "IDLE",
            FaceState::Listening => "LISTEN",
            FaceState::Thinking => "THINK",
            FaceState::Speaking => "SPEAK",
            FaceState::Acting => "ACT",
            FaceState::Alert => "ALERT",
            FaceState::Offline => "OFFLINE",
        }
    }

    /// Primary accent, used for rings/ticks/labels.
    pub fn accent(&self) -> Color {
        match self {
            FaceState::Idle => Color::from_rgba8(90, 220, 240, 255),
            FaceState::Listening => Color::from_rgba8(140, 240, 255, 255),
            FaceState::Thinking => Color::from_rgba8(120, 180, 255, 255),
            FaceState::Speaking => Color::from_rgba8(200, 140, 255, 255),
            FaceState::Acting => Color::from_rgba8(140, 240, 160, 255),
            FaceState::Alert => Color::from_rgba8(255, 90, 90, 255),
            FaceState::Offline => Color::from_rgba8(130, 130, 130, 255),
        }
    }

    /// Dim variant for inactive marks.
    pub fn accent_dim(&self) -> Color {
        let mut c = self.accent();
        c.set_alpha(0.35);
        c
    }
}

pub struct Renderer {
    face_image: Option<Pixmap>,
}

impl Renderer {
    pub fn new(face_image_bytes: Option<&[u8]>) -> Self {
        let face_image = face_image_bytes.and_then(|bytes| decode_png_to_pixmap(bytes));
        Self { face_image }
    }

    /// Paint the whole HUD into `canvas`. Overwrites existing contents.
    pub fn render(&self, canvas: &mut Pixmap, state: FaceState, tick: u64) {
        canvas.fill(Color::from_rgba8(10, 14, 24, 220));

        let w = canvas.width() as f32;
        let h = canvas.height() as f32;
        let cx = w * 0.5;
        let cy = h * 0.5 - 10.0;
        let r_outer = (w.min(h) * 0.44).max(40.0);
        let r_image = r_outer * 0.72;

        self.draw_glow(canvas, cx, cy, r_outer, state);
        if let Some(img) = &self.face_image {
            self.draw_face_image(canvas, img, cx, cy, r_image, state);
        } else {
            self.draw_fallback_head(canvas, cx, cy, r_image, state);
        }
        self.draw_outer_ring(canvas, cx, cy, r_outer, state, tick);
        self.draw_inner_ring(canvas, cx, cy, r_outer * 0.90, state, tick);
        self.draw_ticks(canvas, cx, cy, r_outer, state, tick);
        self.draw_cardinals(canvas, cx, cy, r_outer * 1.07, state, tick);

        match state {
            FaceState::Listening => self.draw_scan_line(canvas, cx, cy, r_image, state, tick),
            FaceState::Speaking => self.draw_waveform(canvas, cx, cy + r_image * 0.9, r_image, state, tick),
            _ => {}
        }

        self.draw_state_label(canvas, cx, h - 14.0, state);
    }

    fn draw_glow(&self, canvas: &mut Pixmap, cx: f32, cy: f32, r: f32, state: FaceState) {
        let mut paint = Paint::default();
        let mut c = state.accent();
        c.set_alpha(0.08);
        paint.set_color(c);
        paint.anti_alias = true;
        if let Some(path) = PathBuilder::from_circle(cx, cy, r * 1.25) {
            canvas.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
        }
    }

    fn draw_face_image(&self, canvas: &mut Pixmap, img: &Pixmap, cx: f32, cy: f32, r: f32, state: FaceState) {
        let size = (r * 2.0) as u32;
        if size == 0 {
            return;
        }
        // Scale the face image into a side × side pixmap.
        let scale = size as f32 / img.width() as f32;
        let mut scaled = match Pixmap::new(size, size) {
            Some(p) => p,
            None => return,
        };
        let pattern = tiny_skia::Pattern::new(
            img.as_ref(),
            tiny_skia::SpreadMode::Pad,
            FilterQuality::Bilinear,
            1.0,
            Transform::from_scale(scale, scale),
        );
        let mut paint = Paint {
            shader: pattern,
            ..Paint::default()
        };
        paint.anti_alias = true;
        if let Some(circle) = PathBuilder::from_circle(size as f32 * 0.5, size as f32 * 0.5, size as f32 * 0.5) {
            scaled.fill_path(&circle, &paint, FillRule::Winding, Transform::identity(), None);
        }

        // Tint the scaled image to match the state hue — mix mode is a
        // multiply-ish blend that keeps the face silhouette while
        // shifting its color temperature.
        tint_pixmap(&mut scaled, state);

        let x = (cx - r) as i32;
        let y = (cy - r) as i32;
        canvas.draw_pixmap(
            x,
            y,
            scaled.as_ref(),
            &PixmapPaint {
                opacity: match state {
                    FaceState::Offline => 0.5,
                    _ => 1.0,
                },
                blend_mode: BlendMode::SourceOver,
                quality: FilterQuality::Bilinear,
            },
            Transform::identity(),
            None,
        );
    }

    fn draw_fallback_head(&self, canvas: &mut Pixmap, cx: f32, cy: f32, r: f32, state: FaceState) {
        // Simple schematic head: two eyes + a jaw bar. Used when the
        // png asset is not shipped (e.g. standalone test run).
        let accent = state.accent();
        let dim = state.accent_dim();

        let mut body = Paint::default();
        body.set_color(dim);
        body.anti_alias = true;
        let rect = tiny_skia::Rect::from_xywh(cx - r * 0.6, cy - r * 0.7, r * 1.2, r * 1.4).unwrap();
        let h = PathBuilder::from_rect(rect);
        canvas.fill_path(&h, &body, FillRule::Winding, Transform::identity(), None);

        let mut eye = Paint::default();
        eye.set_color(accent);
        eye.anti_alias = true;
        for ex in [-r * 0.22, r * 0.22] {
            if let Some(p) = PathBuilder::from_circle(cx + ex, cy - r * 0.15, r * 0.12) {
                canvas.fill_path(&p, &eye, FillRule::Winding, Transform::identity(), None);
            }
        }
    }

    fn draw_outer_ring(&self, canvas: &mut Pixmap, cx: f32, cy: f32, r: f32, state: FaceState, tick: u64) {
        let speed = match state {
            FaceState::Thinking => 0.08,
            FaceState::Acting => 0.05,
            FaceState::Alert => 0.12,
            _ => 0.02,
        };
        let phase = (tick as f32) * speed;
        let mut paint = Paint::default();
        paint.set_color(state.accent());
        paint.anti_alias = true;

        let dash = tiny_skia::StrokeDash::new(vec![6.0, 4.0], phase).unwrap();
        let stroke = Stroke { width: 1.4, dash: Some(dash), ..Stroke::default() };
        if let Some(path) = PathBuilder::from_circle(cx, cy, r) {
            canvas.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    }

    fn draw_inner_ring(&self, canvas: &mut Pixmap, cx: f32, cy: f32, r: f32, state: FaceState, tick: u64) {
        let speed = match state {
            FaceState::Thinking => -0.11,
            _ => -0.03,
        };
        let phase = (tick as f32) * speed;
        let mut paint = Paint::default();
        paint.set_color(state.accent_dim());
        paint.anti_alias = true;
        let dash = tiny_skia::StrokeDash::new(vec![3.0, 7.0], phase).unwrap();
        let stroke = Stroke { width: 1.0, dash: Some(dash), ..Stroke::default() };
        if let Some(path) = PathBuilder::from_circle(cx, cy, r) {
            canvas.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    }

    fn draw_ticks(&self, canvas: &mut Pixmap, cx: f32, cy: f32, r_outer: f32, state: FaceState, _tick: u64) {
        let mut paint = Paint::default();
        paint.set_color(state.accent());
        paint.anti_alias = true;
        let stroke = Stroke { width: 1.1, ..Stroke::default() };

        for i in 0..36 {
            let angle = (i as f32) * (TAU / 36.0);
            let long_tick = i % 3 == 0;
            let r_in = if long_tick { r_outer * 1.03 } else { r_outer * 1.06 };
            let r_out = r_outer * 1.12;
            let (sx, sy) = (cx + angle.cos() * r_in, cy + angle.sin() * r_in);
            let (ex, ey) = (cx + angle.cos() * r_out, cy + angle.sin() * r_out);
            let mut pb = PathBuilder::new();
            pb.move_to(sx, sy);
            pb.line_to(ex, ey);
            if let Some(path) = pb.finish() {
                let mut p = paint.clone();
                if !long_tick {
                    let mut c = state.accent_dim();
                    c.set_alpha(0.35);
                    p.set_color(c);
                }
                canvas.stroke_path(&path, &p, &stroke, Transform::identity(), None);
            }
        }
    }

    fn draw_cardinals(&self, canvas: &mut Pixmap, cx: f32, cy: f32, r: f32, state: FaceState, tick: u64) {
        let mut paint = Paint::default();
        paint.anti_alias = true;
        let bright = state.accent();
        let dim = state.accent_dim();
        let highlight = match state {
            FaceState::Acting => ((tick / 3) as usize) % 4,
            FaceState::Alert => if tick % 2 == 0 { 4 } else { 5 },
            _ => 6,
        };

        for (i, (dx, dy)) in [(0.0, -1.0), (1.0, 0.0), (0.0, 1.0), (-1.0, 0.0)].iter().enumerate() {
            let x = cx + dx * r;
            let y = cy + dy * r;
            let c = if i == highlight { bright } else { dim };
            paint.set_color(c);
            if let Some(p) = PathBuilder::from_circle(x, y, 3.0) {
                canvas.fill_path(&p, &paint, FillRule::Winding, Transform::identity(), None);
            }
        }
    }

    fn draw_scan_line(&self, canvas: &mut Pixmap, cx: f32, cy: f32, r: f32, state: FaceState, tick: u64) {
        let y_phase = ((tick % 60) as f32 / 60.0) * (r * 2.0);
        let y = cy - r + y_phase;
        let mut paint = Paint::default();
        let mut c = state.accent();
        c.set_alpha(0.65);
        paint.set_color(c);
        paint.anti_alias = true;
        let stroke = Stroke { width: 2.0, ..Stroke::default() };
        let mut pb = PathBuilder::new();
        pb.move_to(cx - r * 0.95, y);
        pb.line_to(cx + r * 0.95, y);
        if let Some(path) = pb.finish() {
            canvas.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    }

    fn draw_waveform(&self, canvas: &mut Pixmap, cx: f32, y: f32, half_w: f32, state: FaceState, tick: u64) {
        let bars = 7;
        let mut paint = Paint::default();
        paint.set_color(state.accent());
        paint.anti_alias = true;
        let gap = half_w * 1.4 / bars as f32;
        let x0 = cx - (gap * (bars - 1) as f32) * 0.5;
        for i in 0..bars {
            let phase = (tick as f32 * 0.3) + (i as f32 * 0.9);
            let amp = (phase.sin().abs() * 10.0 + 2.0).max(2.0);
            let bx = x0 + gap * i as f32;
            if let Some(rect) = tiny_skia::Rect::from_xywh(bx - 2.0, y - amp, 4.0, amp * 2.0) {
                let path = PathBuilder::from_rect(rect);
                canvas.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
            }
        }
    }

    fn draw_state_label(&self, canvas: &mut Pixmap, cx: f32, y: f32, state: FaceState) {
        // We don't ship a font (no system cairo); render the label as a
        // framed badge: pill background + a dash/pip per letter.
        let mut paint = Paint::default();
        paint.set_color(state.accent());
        paint.anti_alias = true;
        let label = state.label();
        let w = (label.len() as f32) * 7.0 + 20.0;
        let rect = tiny_skia::Rect::from_xywh(cx - w * 0.5, y - 8.0, w, 14.0).unwrap();
        let pill = PathBuilder::from_rect(rect);
        {
            let mut bg = Paint::default();
            let mut c = state.accent();
            c.set_alpha(0.18);
            bg.set_color(c);
            bg.anti_alias = true;
            canvas.fill_path(&pill, &bg, FillRule::Winding, Transform::identity(), None);
            let stroke = Stroke { width: 1.0, ..Stroke::default() };
            canvas.stroke_path(&pill, &paint, &stroke, Transform::identity(), None);
        }
        // One pip per letter.
        let gap = (w - 10.0) / label.len() as f32;
        for i in 0..label.len() {
            let x = cx - w * 0.5 + 5.0 + gap * (i as f32 + 0.5);
            if let Some(c) = PathBuilder::from_circle(x, y - 1.0, 1.6) {
                canvas.fill_path(&c, &paint, FillRule::Winding, Transform::identity(), None);
            }
        }
    }
}

/// Multiply every opaque pixel of `pm` by the state accent so the face
/// gets a uniform tint for the current state. Keeps luminance.
fn tint_pixmap(pm: &mut Pixmap, state: FaceState) {
    let accent = state.accent();
    let (tr, tg, tb) = ((accent.red() * 255.0) as u16, (accent.green() * 255.0) as u16, (accent.blue() * 255.0) as u16);
    for px in pm.pixels_mut() {
        if px.alpha() == 0 {
            continue;
        }
        // Mix 60/40 toward the state tint.
        let r = ((px.red() as u16) * 2 + tr * 3) / 5;
        let g = ((px.green() as u16) * 2 + tg * 3) / 5;
        let b = ((px.blue() as u16) * 2 + tb * 3) / 5;
        *px = PremultipliedColorU8::from_rgba(r as u8, g as u8, b as u8, px.alpha())
            .unwrap_or(*px);
    }
}

fn decode_png_to_pixmap(bytes: &[u8]) -> Option<Pixmap> {
    let decoder = png::Decoder::new(bytes);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    let (w, h) = (info.width, info.height);

    // tiny-skia expects pre-multiplied RGBA.
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => {
            let mut out = Vec::with_capacity((w * h * 4) as usize);
            for chunk in buf.chunks(3) {
                out.extend_from_slice(chunk);
                out.push(255);
            }
            out
        }
        png::ColorType::Grayscale => {
            let mut out = Vec::with_capacity((w * h * 4) as usize);
            for &g in &buf {
                out.extend_from_slice(&[g, g, g, 255]);
            }
            out
        }
        png::ColorType::GrayscaleAlpha => {
            let mut out = Vec::with_capacity((w * h * 4) as usize);
            for chunk in buf.chunks(2) {
                let (g, a) = (chunk[0], chunk[1]);
                out.extend_from_slice(&[g, g, g, a]);
            }
            out
        }
        _ => return None,
    };

    // Premultiply alpha so tiny-skia sampling blends correctly.
    let mut pm = Pixmap::new(w, h)?;
    let dst = pm.pixels_mut();
    for (i, chunk) in rgba.chunks(4).enumerate() {
        let a = chunk[3];
        let af = a as u16;
        let r = ((chunk[0] as u16) * af / 255) as u8;
        let g = ((chunk[1] as u16) * af / 255) as u8;
        let b = ((chunk[2] as u16) * af / 255) as u8;
        if let Some(p) = PremultipliedColorU8::from_rgba(r, g, b, a) {
            dst[i] = p;
        }
    }
    Some(pm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renderer_runs_without_face_image() {
        let r = Renderer::new(None);
        let mut canvas = Pixmap::new(240, 260).unwrap();
        for s in [FaceState::Idle, FaceState::Listening, FaceState::Thinking,
                  FaceState::Speaking, FaceState::Acting, FaceState::Alert, FaceState::Offline] {
            for t in 0..5 {
                r.render(&mut canvas, s, t);
            }
        }
    }

    #[test]
    fn states_have_distinct_accents() {
        let a = FaceState::Idle.accent();
        let b = FaceState::Alert.accent();
        assert_ne!((a.red(), a.green(), a.blue()), (b.red(), b.green(), b.blue()));
    }

    #[test]
    fn png_decode_round_trip() {
        // Build a tiny 2x2 red PNG in memory and decode it back.
        let mut png_bytes = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut png_bytes, 2, 2);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(&[
                255,0,0,255, 255,0,0,255,
                255,0,0,255, 255,0,0,255,
            ]).unwrap();
        }
        let pm = decode_png_to_pixmap(&png_bytes).unwrap();
        assert_eq!(pm.width(), 2);
        assert_eq!(pm.height(), 2);
    }
}
