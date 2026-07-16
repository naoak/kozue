//! Deterministic SVG renderer for the Scene IR.
//!
//! Float precision is fixed to two decimals and attribute order is fixed so
//! that identical scenes always produce byte-identical SVG.

use std::fmt::Write;

use kozue_ir::{Path, Rect, Scene, SceneItem, StrokeStyle, StrokeWeight, Text, TextAlign};

const MARGIN: f64 = 20.0;
const FONT_FAMILY: &str = "DejaVu Sans";

/// Render a [`Scene`] to an SVG document string.
///
/// The layout owns the scene bounds: items are already normalized to a
/// `(0, 0)` origin and `Scene.width`/`Scene.height` cover every item
/// (including text and arrowheads), so the viewBox is derived directly
/// from them plus a fixed margin.
pub fn render(scene: &Scene) -> String {
    let vb_x = -MARGIN;
    let vb_y = -MARGIN;
    let vb_w = scene.width + 2.0 * MARGIN;
    let vb_h = scene.height + 2.0 * MARGIN;

    let mut s = String::new();
    let _ = writeln!(
        s,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{} {} {} {}\" width=\"{}\" height=\"{}\">",
        f(vb_x),
        f(vb_y),
        f(vb_w),
        f(vb_h),
        f(vb_w),
        f(vb_h),
    );

    // White background rect covering the whole viewBox.
    let _ = writeln!(
        s,
        "  <rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"#ffffff\"/>",
        f(vb_x),
        f(vb_y),
        f(vb_w),
        f(vb_h),
    );

    for item in &scene.items {
        render_item(&mut s, item, 1);
    }

    s.push_str("</svg>\n");
    s
}

fn render_item(s: &mut String, item: &SceneItem, depth: usize) {
    let indent = "  ".repeat(depth);
    match item {
        SceneItem::Rect(r) => render_rect(s, r, &indent),
        SceneItem::Path(p) => render_path(s, p, &indent),
        SceneItem::Text(t) => render_text(s, t, &indent),
        SceneItem::Group(g) => {
            let _ = writeln!(s, "{indent}<g data-name=\"{}\">", escape(&g.name));
            for child in &g.items {
                render_item(s, child, depth + 1);
            }
            let _ = writeln!(s, "{indent}</g>");
        }
        _ => {} // future SceneItem variants: silently skip
    }
}

fn render_rect(s: &mut String, r: &Rect, indent: &str) {
    let _ = writeln!(
        s,
        "{indent}<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"{}\" ry=\"{}\" fill=\"#ffffff\" stroke=\"#000000\" stroke-width=\"1.50\"/>",
        f(r.x),
        f(r.y),
        f(r.width),
        f(r.height),
        f(r.rx),
        f(r.rx),
    );
}

fn render_path(s: &mut String, p: &Path, indent: &str) {
    let mut pts = String::new();
    for (i, (x, y)) in p.points.iter().enumerate() {
        if i > 0 {
            pts.push(' ');
        }
        let _ = write!(pts, "{},{}", f(*x), f(*y));
    }
    if p.filled {
        let _ = writeln!(
            s,
            "{indent}<polygon points=\"{pts}\" fill=\"#000000\" stroke=\"none\"/>",
        );
    } else {
        let width = match p.weight {
            StrokeWeight::Thick => f(3.0),
            StrokeWeight::Normal => f(1.5),
            // `StrokeWeight` is `#[non_exhaustive]`: treat any future variant
            // as the normal weight rather than panic.
            _ => f(1.5),
        };
        let dasharray = match p.stroke {
            StrokeStyle::Solid => String::new(),
            StrokeStyle::Dashed => " stroke-dasharray=\"6 4\"".to_string(),
            StrokeStyle::Dotted => format!(" stroke-dasharray=\"{} {}\"", f(1.5), f(3.0)),
            // `StrokeStyle` is `#[non_exhaustive]`: fall back to the fine dotted
            // pattern for any future variant rather than panic.
            _ => format!(" stroke-dasharray=\"{} {}\"", f(1.5), f(3.0)),
        };
        let _ = writeln!(
            s,
            "{indent}<polyline points=\"{pts}\" fill=\"none\" stroke=\"#000000\" stroke-width=\"{width}\"{dasharray}/>",
        );
    }
}

fn render_text(s: &mut String, t: &Text, indent: &str) {
    let anchor = match t.align {
        TextAlign::Start => "start",
        TextAlign::Middle => "middle",
        TextAlign::End => "end",
        _ => "start", // future TextAlign variants: fallback to start
    };
    let _ = writeln!(
        s,
        "{indent}<text x=\"{}\" y=\"{}\" font-family=\"{}\" font-size=\"{}\" text-anchor=\"{}\" fill=\"#000000\">{}</text>",
        f(t.x),
        f(t.y),
        FONT_FAMILY,
        f(t.size),
        anchor,
        escape(&t.content),
    );
}

/// Format a float with fixed two-decimal precision (avoiding `-0.00`).
fn f(v: f64) -> String {
    let s = format!("{:.2}", v);
    // Normalise negative zero.
    if s == "-0.00" {
        "0.00".to_string()
    } else {
        s
    }
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_scene_renders_valid_svg() {
        let scene = Scene {
            width: 0.0,
            height: 0.0,
            items: vec![],
        };
        let svg = render(&scene);
        assert!(svg.starts_with("<svg"));
        assert!(svg.trim_end().ends_with("</svg>"));
    }

    #[test]
    fn float_formatting_is_fixed() {
        assert_eq!(f(1.005), "1.00");
        assert_eq!(f(-0.0), "0.00");
        assert_eq!(f(12.3456), "12.35");
    }
}
