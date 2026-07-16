//! Deterministic PNG rasterizer for the Scene IR.
//!
//! ## Rationale
//!
//! SVG is the primary output format; PNG is provided for environments that
//! cannot display SVG (e.g. some markdown viewers, image comparison tools).
//! The rasterizer reads only the Scene IR — it is independent of any frontend
//! or layout engine.
//!
//! ## Determinism
//!
//! Rendering is reproducible for a **fixed build target**:
//! - No HashMap anywhere; iteration order follows the Scene IR item order.
//! - `tiny_skia` produces deterministic pixel output for a given target.
//! - `Pixmap::encode_png()` is used directly — tiny-skia's built-in PNG
//!   encoder (pure-Rust `png` + `miniz_oxide`) uses a fixed filter and writes
//!   no timestamp chunk, so encoded bytes are stable across runs and processes.
//!
//! **Cross-platform caveat:** `tiny_skia` enables SIMD by default and its
//! antialiasing runs in `f32`; the SIMD code paths are not guaranteed to be
//! bit-identical across CPU architectures (e.g. x86_64 vs aarch64). The
//! committed PNG goldens are therefore pinned to one canonical target —
//! byte-identity holds across runs and processes on the same target triple and
//! toolchain, not necessarily across architectures. This mirrors the honesty
//! of the Japanese-glyph limitation noted below.
//!
//! ## Japanese glyph limitation
//!
//! The embedded DejaVu Sans font does not include CJK glyphs. Characters with
//! no glyph outline are rendered as blank space (the pen still advances by
//! 1 em so overall text width matches the measured layout width). This matches
//! the SVG output, which delegates glyph rendering to the browser's font stack.

use kozue_ir::{Path, Rect, Scene, SceneItem, StrokeStyle, StrokeWeight, Text, TextAlign};
use kozue_text::{glyph_advance_units, glyph_outline, units_per_em, OutlineCmd};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};

const MARGIN: f64 = 20.0;

/// An error that prevents a [`Scene`] from being rasterized.
///
/// The layout engine is expected to always produce finite, non-negative,
/// reasonably-sized bounds, so these cases indicate a malformed scene rather
/// than ordinary input — but the renderer surfaces them explicitly instead of
/// panicking or silently producing a truncated image (which would violate the
/// project's "never silently destroy data" principle).
#[derive(Debug, Clone, PartialEq)]
pub enum RenderError {
    /// `scene.width` / `scene.height` was NaN, infinite, or negative.
    InvalidBounds { width: f64, height: f64 },
    /// The pixel canvas is too large for `tiny_skia` to allocate.
    CanvasTooLarge { width: u32, height: u32 },
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::InvalidBounds { width, height } => write!(
                f,
                "scene bounds must be finite and non-negative (got {width} x {height})"
            ),
            RenderError::CanvasTooLarge { width, height } => {
                write!(f, "PNG canvas {width}x{height} px is too large to allocate")
            }
        }
    }
}

impl std::error::Error for RenderError {}

/// Render a laid-out [`Scene`] to PNG bytes.
///
/// Deterministic: identical scenes always produce byte-identical output on a
/// fixed target (see module docs). 1 scene unit = 1 pixel (no scaling).
///
/// Returns [`RenderError`] for malformed scenes (non-finite/negative bounds or
/// a canvas too large to allocate) rather than panicking.
pub fn render(scene: &Scene) -> Result<Vec<u8>, RenderError> {
    // Reject non-finite / negative bounds loudly instead of letting the
    // `f64 -> u32` cast saturate to a tiny canvas that silently drops content.
    if !scene.width.is_finite()
        || !scene.height.is_finite()
        || scene.width < 0.0
        || scene.height < 0.0
    {
        return Err(RenderError::InvalidBounds {
            width: scene.width,
            height: scene.height,
        });
    }

    // `ceil` matches the SVG viewBox extent (`scene.{width,height} + 2*MARGIN`)
    // without clipping fractional bounds, and — unlike a `+1` pad — keeps an
    // integer-sized scene the same size as the SVG canvas. A huge but finite
    // bound saturates to `u32::MAX`, which `Pixmap::new` then rejects.
    let width_px = (scene.width + 2.0 * MARGIN).ceil() as u32;
    let height_px = (scene.height + 2.0 * MARGIN).ceil() as u32;

    let mut pixmap = match Pixmap::new(width_px, height_px) {
        Some(p) => p,
        None => {
            return Err(RenderError::CanvasTooLarge {
                width: width_px,
                height: height_px,
            })
        }
    };

    // Fill white background.
    pixmap.fill(tiny_skia::Color::WHITE);

    // All scene coords are offset by (+MARGIN, +MARGIN).
    let ox = MARGIN as f32;
    let oy = MARGIN as f32;

    for item in &scene.items {
        render_item(&mut pixmap, item, ox, oy);
    }

    // Encoding a valid, non-empty pixmap does not depend on scene data and does
    // not fail in practice, so a failure here is a true internal invariant break.
    Ok(pixmap.encode_png().expect("PNG encoding must succeed"))
}

fn render_item(pixmap: &mut Pixmap, item: &SceneItem, ox: f32, oy: f32) {
    match item {
        SceneItem::Rect(r) => render_rect(pixmap, r, ox, oy),
        SceneItem::Path(p) => render_path(pixmap, p, ox, oy),
        SceneItem::Text(t) => render_text(pixmap, t, ox, oy),
        SceneItem::Group(g) => {
            for child in &g.items {
                render_item(pixmap, child, ox, oy);
            }
        }
        _ => {} // future variants: silently skip
    }
}

fn render_rect(pixmap: &mut Pixmap, r: &Rect, ox: f32, oy: f32) {
    let x = r.x as f32 + ox;
    let y = r.y as f32 + oy;
    let w = r.width as f32;
    let h = r.height as f32;

    // Clamp rx to at most half of width/height.
    let rx = (r.rx as f32).min(w / 2.0).min(h / 2.0);

    let path = if rx > 0.0 {
        build_rounded_rect(x, y, w, h, rx)
    } else {
        build_rect_path(x, y, w, h)
    };

    let path = match path {
        Some(p) => p,
        None => return,
    };

    // Fill white.
    let mut fill_paint = Paint::default();
    fill_paint.set_color(tiny_skia::Color::WHITE);
    fill_paint.anti_alias = true;
    pixmap.fill_path(
        &path,
        &fill_paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );

    // Stroke black, width 1.5.
    let mut stroke_paint = Paint::default();
    stroke_paint.set_color(tiny_skia::Color::BLACK);
    stroke_paint.anti_alias = true;
    let stroke = Stroke {
        width: 1.5,
        ..Stroke::default()
    };
    pixmap.stroke_path(&path, &stroke_paint, &stroke, Transform::identity(), None);
}

fn build_rect_path(x: f32, y: f32, w: f32, h: f32) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    pb.move_to(x, y);
    pb.line_to(x + w, y);
    pb.line_to(x + w, y + h);
    pb.line_to(x, y + h);
    pb.close();
    pb.finish()
}

fn build_rounded_rect(x: f32, y: f32, w: f32, h: f32, rx: f32) -> Option<tiny_skia::Path> {
    // Cubic bezier approximation constant for quarter circle.
    const K: f32 = 0.552_284_8;
    let kx = rx * K;
    // For simplicity use rx for ry as well (uniform corner radius).
    let ry = rx;
    let ky = ry * K;

    let mut pb = PathBuilder::new();
    // Top edge: start after top-left corner.
    pb.move_to(x + rx, y);
    // Top edge to top-right corner start.
    pb.line_to(x + w - rx, y);
    // Top-right corner.
    pb.cubic_to(x + w - rx + kx, y, x + w, y + ry - ky, x + w, y + ry);
    // Right edge.
    pb.line_to(x + w, y + h - ry);
    // Bottom-right corner.
    pb.cubic_to(
        x + w,
        y + h - ry + ky,
        x + w - rx + kx,
        y + h,
        x + w - rx,
        y + h,
    );
    // Bottom edge.
    pb.line_to(x + rx, y + h);
    // Bottom-left corner.
    pb.cubic_to(x + rx - kx, y + h, x, y + h - ry + ky, x, y + h - ry);
    // Left edge.
    pb.line_to(x, y + ry);
    // Top-left corner.
    pb.cubic_to(x, y + ry - ky, x + rx - kx, y, x + rx, y);
    pb.close();
    pb.finish()
}

fn render_path(pixmap: &mut Pixmap, p: &Path, ox: f32, oy: f32) {
    if p.points.len() < 2 {
        return;
    }

    let mut pb = PathBuilder::new();
    let (x0, y0) = p.points[0];
    pb.move_to(x0 as f32 + ox, y0 as f32 + oy);
    for &(x, y) in &p.points[1..] {
        pb.line_to(x as f32 + ox, y as f32 + oy);
    }
    if p.filled {
        pb.close();
    }

    let path = match pb.finish() {
        Some(p) => p,
        None => return,
    };

    if p.filled {
        // Arrowhead: fill black.
        let mut paint = Paint::default();
        paint.set_color(tiny_skia::Color::BLACK);
        paint.anti_alias = true;
        pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    } else {
        // Polyline: stroke black.
        let mut paint = Paint::default();
        paint.set_color(tiny_skia::Color::BLACK);
        paint.anti_alias = true;
        let dash = match p.stroke {
            StrokeStyle::Solid => None,
            StrokeStyle::Dashed => tiny_skia::StrokeDash::new(vec![6.0, 4.0], 0.0),
            StrokeStyle::Dotted => tiny_skia::StrokeDash::new(vec![1.5, 3.0], 0.0),
            // `StrokeStyle` is `#[non_exhaustive]`: fall back to the fine dotted
            // pattern for any future variant rather than panic.
            _ => tiny_skia::StrokeDash::new(vec![1.5, 3.0], 0.0),
        };
        let width = match p.weight {
            StrokeWeight::Thick => 3.0,
            StrokeWeight::Normal => 1.5,
            // `StrokeWeight` is `#[non_exhaustive]`: treat any future variant
            // as the normal weight rather than panic.
            _ => 1.5,
        };
        let stroke = Stroke {
            width,
            dash,
            ..Stroke::default()
        };
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}

fn render_text(pixmap: &mut Pixmap, t: &Text, ox: f32, oy: f32) {
    let upm = units_per_em();
    let scale = t.size / upm;

    // Compute pen start x based on alignment.
    let pen_start_x = match t.align {
        TextAlign::Start => t.x,
        TextAlign::Middle => t.x - t.text_width / 2.0,
        TextAlign::End => t.x - t.text_width,
        _ => t.x, // future variants: fallback to start
    };

    let mut pen_x = pen_start_x;
    let baseline_y = t.y;

    for ch in t.content.chars() {
        let cmds = glyph_outline(ch);
        let advance = glyph_advance_units(ch);

        if !cmds.is_empty() {
            // Build path for this glyph.
            // Font space is y-up; raster is y-down. Flip y.
            let mut pb = PathBuilder::new();
            for cmd in &cmds {
                match *cmd {
                    OutlineCmd::MoveTo { x, y } => {
                        let sx = (pen_x + x as f64 * scale) as f32 + ox;
                        let sy = (baseline_y - y as f64 * scale) as f32 + oy;
                        pb.move_to(sx, sy);
                    }
                    OutlineCmd::LineTo { x, y } => {
                        let sx = (pen_x + x as f64 * scale) as f32 + ox;
                        let sy = (baseline_y - y as f64 * scale) as f32 + oy;
                        pb.line_to(sx, sy);
                    }
                    OutlineCmd::QuadTo { x1, y1, x, y } => {
                        let sx1 = (pen_x + x1 as f64 * scale) as f32 + ox;
                        let sy1 = (baseline_y - y1 as f64 * scale) as f32 + oy;
                        let sx = (pen_x + x as f64 * scale) as f32 + ox;
                        let sy = (baseline_y - y as f64 * scale) as f32 + oy;
                        pb.quad_to(sx1, sy1, sx, sy);
                    }
                    OutlineCmd::CurveTo {
                        x1,
                        y1,
                        x2,
                        y2,
                        x,
                        y,
                    } => {
                        let sx1 = (pen_x + x1 as f64 * scale) as f32 + ox;
                        let sy1 = (baseline_y - y1 as f64 * scale) as f32 + oy;
                        let sx2 = (pen_x + x2 as f64 * scale) as f32 + ox;
                        let sy2 = (baseline_y - y2 as f64 * scale) as f32 + oy;
                        let sx = (pen_x + x as f64 * scale) as f32 + ox;
                        let sy = (baseline_y - y as f64 * scale) as f32 + oy;
                        pb.cubic_to(sx1, sy1, sx2, sy2, sx, sy);
                    }
                    OutlineCmd::Close => {
                        pb.close();
                    }
                }
            }

            if let Some(path) = pb.finish() {
                let mut paint = Paint::default();
                paint.set_color(tiny_skia::Color::BLACK);
                paint.anti_alias = true;
                pixmap.fill_path(
                    &path,
                    &paint,
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
        }

        // Advance pen (even for missing glyphs — blank space).
        pen_x += advance * scale;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kozue_ir::{Rect as IrRect, SceneItem};

    fn empty_scene() -> Scene {
        Scene {
            width: 0.0,
            height: 0.0,
            items: vec![],
        }
    }

    fn rect_scene(rx: f64) -> Scene {
        Scene {
            width: 100.0,
            height: 60.0,
            items: vec![SceneItem::Rect(IrRect {
                x: 10.0,
                y: 10.0,
                width: 80.0,
                height: 40.0,
                rx,
            })],
        }
    }

    #[test]
    fn empty_scene_renders_valid_png() {
        let png = render(&empty_scene()).expect("empty scene must render");
        // PNG magic bytes: \x89PNG\r\n\x1a\n
        assert!(
            png.starts_with(b"\x89PNG"),
            "output must start with PNG magic bytes"
        );
    }

    #[test]
    fn scene_with_rect_renders_non_empty() {
        let png = render(&rect_scene(5.0)).expect("rect scene must render");
        assert!(!png.is_empty());
        assert!(png.starts_with(b"\x89PNG"));
    }

    #[test]
    fn same_scene_produces_identical_bytes() {
        let png1 = render(&rect_scene(0.0)).expect("render 1");
        let png2 = render(&rect_scene(0.0)).expect("render 2");
        assert_eq!(
            png1, png2,
            "rendering the same scene twice must be byte-identical"
        );
    }

    #[test]
    fn rounded_and_sharp_rects_differ() {
        // Exercises the rounded-corner bezier geometry: a rounded rect must
        // rasterize differently from a sharp one.
        let sharp = render(&rect_scene(0.0)).expect("sharp render");
        let rounded = render(&rect_scene(12.0)).expect("rounded render");
        assert_ne!(
            sharp, rounded,
            "rounded corners must change the raster output"
        );
    }

    #[test]
    fn non_finite_bounds_are_rejected() {
        let scene = Scene {
            width: f64::NAN,
            height: 10.0,
            items: vec![],
        };
        assert!(matches!(
            render(&scene),
            Err(RenderError::InvalidBounds { .. })
        ));
    }

    #[test]
    fn negative_bounds_are_rejected() {
        let scene = Scene {
            width: -5.0,
            height: 10.0,
            items: vec![],
        };
        assert!(matches!(
            render(&scene),
            Err(RenderError::InvalidBounds { .. })
        ));
    }

    #[test]
    fn oversized_canvas_errors_gracefully() {
        // A finite but enormous bound saturates the u32 cast; Pixmap::new
        // rejects it, so we get a clean error instead of a panic or huge alloc.
        let scene = Scene {
            width: 1e12,
            height: 1e12,
            items: vec![],
        };
        assert!(matches!(
            render(&scene),
            Err(RenderError::CanvasTooLarge { .. })
        ));
    }
}
