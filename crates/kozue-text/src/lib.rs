//! Text measurement using an embedded DejaVu Sans font.
//!
//! For M0 we sum per-glyph horizontal advances (no kerning). Characters that
//! are not present in the font (e.g. Japanese) fall back to a 1 em advance.
//!
//! The per-glyph advance table is built once via [`OnceLock`] so that
//! [`Face::parse`] is not called on every measurement.

use std::collections::BTreeMap;
use std::sync::OnceLock;

/// The embedded DejaVu Sans font, used for advance-width measurement.
static FONT_BYTES: &[u8] = include_bytes!("../assets/DejaVuSans.ttf");

/// Line-height multiplier applied to `font_size` to get text height.
const LINE_HEIGHT: f64 = 1.2;

/// Cached per-glyph metrics: maps Unicode scalar → advance in font units,
/// plus the `units_per_em` value.
struct FontMetrics {
    /// Advance widths keyed by Unicode scalar value.
    advances: BTreeMap<char, u32>,
    units_per_em: f64,
}

static METRICS: OnceLock<FontMetrics> = OnceLock::new();

fn metrics() -> &'static FontMetrics {
    METRICS.get_or_init(|| {
        let face = ttf_parser::Face::parse(FONT_BYTES, 0)
            .expect("embedded DejaVu Sans font must parse — font data is corrupted");
        let units_per_em = face.units_per_em() as f64;

        // Pre-build advance table for all glyphs that have a Unicode mapping.
        let mut advances: BTreeMap<char, u32> = BTreeMap::new();
        face.tables()
            .cmap
            .iter()
            .flat_map(|cmap| cmap.subtables)
            .filter(|st| st.is_unicode())
            .for_each(|st| {
                st.codepoints(|cp| {
                    if let Some(ch) = char::from_u32(cp) {
                        if let Some(gid) = face.glyph_index(ch) {
                            if let Some(adv) = face.glyph_hor_advance(gid) {
                                advances.insert(ch, adv as u32);
                            }
                        }
                    }
                });
            });

        FontMetrics {
            advances,
            units_per_em,
        }
    })
}

/// Measure the rendered size of `text` at `font_size` (in px).
///
/// Returns `(width, height)` where `height == font_size * 1.2`. Width is the
/// sum of glyph advance widths; missing glyphs use a 1 em fallback.
pub fn measure(text: &str, font_size: f64) -> (f64, f64) {
    let m = metrics();
    let mut width = 0.0_f64;
    for ch in text.chars() {
        let advance_units = m
            .advances
            .get(&ch)
            .copied()
            .unwrap_or(m.units_per_em as u32) as f64;
        width += advance_units / m.units_per_em * font_size;
    }
    (width, font_size * LINE_HEIGHT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_has_zero_width() {
        let (w, h) = measure("", 16.0);
        assert_eq!(w, 0.0);
        assert!((h - 16.0 * 1.2).abs() < 1e-9);
    }

    #[test]
    fn ascii_width_is_positive_and_scales() {
        let (w1, _) = measure("Hello", 16.0);
        let (w2, _) = measure("Hello", 32.0);
        assert!(w1 > 0.0);
        // Width scales linearly with font size.
        assert!((w2 - w1 * 2.0).abs() < 1e-6);
    }

    #[test]
    fn wider_text_is_wider() {
        let (short, _) = measure("i", 16.0);
        let (long, _) = measure("iiiiii", 16.0);
        assert!(long > short);
    }

    #[test]
    fn japanese_uses_one_em_fallback() {
        // Each missing glyph contributes exactly font_size (1 em).
        let font_size = 20.0;
        let (w, _) = measure("開始", font_size);
        assert!((w - 2.0 * font_size).abs() < 1e-9);
    }
}
