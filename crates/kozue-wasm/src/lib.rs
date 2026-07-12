//! WASM entry point for the kozue diagram compiler.
//!
//! This crate exposes three functions to JavaScript via wasm-bindgen:
//! - [`render_svg`]: parse → layout → SVG string.
//! - [`render_png`]: parse → layout → PNG bytes (as `Uint8Array`).
//! - [`check`]: parse-only validation.
//!
//! ## Determinism
//!
//! Determinism is inherent: the same input always produces identical output
//! bytes because the font is embedded (DejaVu Sans, included at compile time
//! by `kozue-text`) and there is no randomness in any pipeline stage. All maps
//! use `IndexMap` or `BTreeMap` — no `HashMap` — so iteration order is stable.
//!
//! ## SVG vs PNG
//!
//! The SVG path delegates glyph rendering to the browser's font stack, so CJK
//! and any character present in the browser's fonts will render correctly.
//! The PNG path bakes DejaVu Sans glyphs at compile time; CJK characters appear
//! as blank space (the pen still advances by 1 em, so layout is not disrupted).
//! This is the same limitation as the native PNG renderer.
//!
//! ## Architecture
//!
//! All real logic lives in plain Rust functions (`*_impl`, `parse_any`) that
//! return `Result<_, String>`. The `#[wasm_bindgen]` exports are thin wrappers
//! that convert the `String` error into a `JsValue`. This means the `*_impl`
//! functions are fully testable via `cargo test` on the native target.

use std::ops::Range;

use wasm_bindgen::prelude::*;

// ---------------------------------------------------------------------------
// Language selector
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum Lang {
    Kozue,
    Mermaid,
    Plantuml,
}

fn parse_lang(lang: &str) -> Result<Lang, String> {
    match lang {
        "kozue" => Ok(Lang::Kozue),
        "mermaid" => Ok(Lang::Mermaid),
        "plantuml" => Ok(Lang::Plantuml),
        other => Err(format!("unknown language: {other}")),
    }
}

// ---------------------------------------------------------------------------
// Diagnostic formatting
// ---------------------------------------------------------------------------

/// The unit a frontend's diagnostic spans are measured in.
///
/// The frontends disagree: `kozue-dsl` is built on `chumsky`, which parses a
/// `char` stream, so its spans are **character indices**. The hand-written
/// `kozue-mermaid` / `kozue-plantuml` scanners emit **byte offsets**. Feeding a
/// character index into byte-based slicing panics on multi-byte input (aborting
/// the whole wasm module across the FFI boundary), so we track the unit and
/// convert to a byte offset explicitly.
#[derive(Debug, Clone, Copy)]
enum SpanUnit {
    Char,
    Byte,
}

/// Convert a span index in `unit` into a byte offset that is guaranteed to be a
/// valid UTF-8 char boundary within `input` (so subsequent slicing never panics).
fn to_byte_offset(input: &str, index: usize, unit: SpanUnit) -> usize {
    match unit {
        SpanUnit::Byte => {
            // Clamp into range, then snap down to the nearest char boundary in
            // case a byte-offset span happens to point inside a codepoint.
            let mut b = index.min(input.len());
            while b > 0 && !input.is_char_boundary(b) {
                b -= 1;
            }
            b
        }
        SpanUnit::Char => input
            .char_indices()
            .map(|(b, _)| b)
            .chain(std::iter::once(input.len()))
            .nth(index)
            .unwrap_or(input.len()),
    }
}

/// Compute the 1-based `(line, column)` of a span `index` (measured in `unit`).
/// Column is counted in characters. Never panics.
fn line_col(input: &str, index: usize, unit: SpanUnit) -> (usize, usize) {
    let byte = to_byte_offset(input, index, unit);
    let line = input[..byte].bytes().filter(|&b| b == b'\n').count() + 1;
    let line_start = input[..byte].rfind('\n').map(|p| p + 1).unwrap_or(0);
    let col = input[line_start..byte].chars().count() + 1;
    (line, col)
}

/// Format a single diagnostic as `"error at line L, column C: message"`.
fn format_diagnostic(input: &str, message: &str, span: &Range<usize>, unit: SpanUnit) -> String {
    let (line, col) = line_col(input, span.start, unit);
    format!("error at line {line}, column {col}: {message}")
}

/// Format a secondary label (e.g. "first declared here") as an indented note.
fn format_secondary(input: &str, message: &str, span: &Range<usize>, unit: SpanUnit) -> String {
    let (line, col) = line_col(input, span.start, unit);
    format!("  note at line {line}, column {col}: {message}")
}

// ---------------------------------------------------------------------------
// Core pipeline — pure Rust, testable on native target
// ---------------------------------------------------------------------------

fn parse_any(input: &str, lang: Lang) -> Result<kozue_ir::Diagram, String> {
    match lang {
        // chumsky spans are character indices.
        Lang::Kozue => kozue_dsl::parse(input).map_err(|errs| {
            let mut lines = Vec::new();
            for e in &errs {
                lines.push(format_diagnostic(
                    input,
                    &e.message,
                    &e.span,
                    SpanUnit::Char,
                ));
                // Surface the secondary label (e.g. "first declared here") so the
                // wasm consumer gets the same information as the CLI's ariadne output.
                if let Some((sec_span, sec_msg)) = &e.secondary {
                    lines.push(format_secondary(input, sec_msg, sec_span, SpanUnit::Char));
                }
            }
            lines.join("\n")
        }),
        // Hand-written scanners emit byte offsets.
        Lang::Mermaid => kozue_mermaid::parse(input).map_err(|errs| {
            errs.iter()
                .map(|e| format_diagnostic(input, &e.message, &e.span, SpanUnit::Byte))
                .collect::<Vec<_>>()
                .join("\n")
        }),
        Lang::Plantuml => kozue_plantuml::parse(input).map_err(|errs| {
            errs.iter()
                .map(|e| format_diagnostic(input, &e.message, &e.span, SpanUnit::Byte))
                .collect::<Vec<_>>()
                .join("\n")
        }),
    }
}

fn svg_impl(input: &str, lang: &str) -> Result<String, String> {
    let lang = parse_lang(lang)?;
    let diagram = parse_any(input, lang)?;
    let scene = kozue_layout::layout(&diagram).map_err(|e| format!("layout failed: {e}"))?;
    Ok(kozue_render_svg::render(&scene))
}

fn png_impl(input: &str, lang: &str) -> Result<Vec<u8>, String> {
    let lang = parse_lang(lang)?;
    let diagram = parse_any(input, lang)?;
    let scene = kozue_layout::layout(&diagram).map_err(|e| format!("layout failed: {e}"))?;
    kozue_render_png::render(&scene).map_err(|e| e.to_string())
}

fn check_impl(input: &str, lang: &str) -> Result<(), String> {
    let lang = parse_lang(lang)?;
    parse_any(input, lang).map(|_| ())
}

// ---------------------------------------------------------------------------
// WASM exports
// ---------------------------------------------------------------------------

/// Parse `input` in `lang` (one of `"kozue"`, `"mermaid"`, `"plantuml"`),
/// lay it out, and return an SVG string.
///
/// On error, rejects with a human-readable diagnostic string.
#[wasm_bindgen]
pub fn render_svg(input: &str, lang: &str) -> Result<String, JsValue> {
    svg_impl(input, lang).map_err(|e| JsValue::from_str(&e))
}

/// Parse `input` in `lang`, lay it out, and return the PNG bytes as a
/// `Uint8Array`.
///
/// On error, rejects with a human-readable diagnostic string.
///
/// Note: CJK characters appear as blank space in PNG output because the
/// embedded DejaVu Sans font does not include CJK glyphs. Use `render_svg`
/// for full Unicode glyph coverage via browser fonts.
#[wasm_bindgen]
pub fn render_png(input: &str, lang: &str) -> Result<Vec<u8>, JsValue> {
    png_impl(input, lang).map_err(|e| JsValue::from_str(&e))
}

/// Parse `input` in `lang` and check for errors without rendering.
///
/// Returns `undefined` on success, or rejects with a diagnostic string on error.
#[wasm_bindgen]
pub fn check(input: &str, lang: &str) -> Result<(), JsValue> {
    check_impl(input, lang).map_err(|e| JsValue::from_str(&e))
}

// ---------------------------------------------------------------------------
// Tests (run on native target via `cargo test -p kozue-wasm`)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const KOZUE_MINIMAL: &str =
        "diagram d {\n  direction down\n  a: \"A\"\n  b: \"B\"\n  a -> b\n}";
    const MERMAID_MINIMAL: &str = "sequenceDiagram\n  participant A as Alice\n  participant B\n  A->>B: hello\n  B-->>A: reply\n";
    const PLANTUML_MINIMAL: &str =
        "@startuml\nparticipant Alice\nparticipant Bob\nAlice -> Bob : hi\n@enduml\n";

    #[test]
    fn svg_impl_kozue_returns_svg_element() {
        let result = svg_impl(KOZUE_MINIMAL, "kozue");
        let svg = result.expect("should produce SVG");
        assert!(
            svg.starts_with("<svg"),
            "expected SVG, got: {}",
            &svg[..svg.len().min(80)]
        );
    }

    #[test]
    fn svg_impl_mermaid_returns_svg_element() {
        let result = svg_impl(MERMAID_MINIMAL, "mermaid");
        let svg = result.expect("should produce SVG for mermaid input");
        assert!(
            svg.starts_with("<svg"),
            "expected SVG, got: {}",
            &svg[..svg.len().min(80)]
        );
    }

    #[test]
    fn svg_impl_plantuml_returns_svg_element() {
        let result = svg_impl(PLANTUML_MINIMAL, "plantuml");
        let svg = result.expect("should produce SVG for plantuml input");
        assert!(
            svg.starts_with("<svg"),
            "expected SVG, got: {}",
            &svg[..svg.len().min(80)]
        );
    }

    #[test]
    fn png_impl_returns_png_magic_bytes() {
        let result = png_impl(KOZUE_MINIMAL, "kozue");
        let bytes = result.expect("should produce PNG bytes");
        assert!(
            bytes.starts_with(b"\x89PNG"),
            "expected PNG magic header, got: {:?}",
            &bytes[..bytes.len().min(8)]
        );
    }

    #[test]
    fn check_impl_valid_input_ok() {
        assert!(check_impl(KOZUE_MINIMAL, "kozue").is_ok());
        assert!(check_impl(MERMAID_MINIMAL, "mermaid").is_ok());
        assert!(check_impl(PLANTUML_MINIMAL, "plantuml").is_ok());
    }

    #[test]
    fn check_impl_invalid_input_err_contains_line() {
        let bad = "this is not valid kozue syntax at all";
        let err = check_impl(bad, "kozue").expect_err("should fail on invalid input");
        assert!(
            err.contains("line"),
            "error message should contain 'line', got: {err}"
        );
    }

    #[test]
    fn line_col_char_unit_multibyte() {
        // "あ" is 1 char / 3 bytes; char index 1 sits just after it.
        assert_eq!(line_col("あx", 1, SpanUnit::Char), (1, 2));
    }

    #[test]
    fn line_col_byte_unit_snaps_to_char_boundary() {
        // Byte offset at the start of `x` → column 2.
        assert_eq!(line_col("あx", 3, SpanUnit::Byte), (1, 2));
        // A byte offset landing inside `あ` snaps back to a boundary (no panic).
        assert_eq!(line_col("あx", 1, SpanUnit::Byte), (1, 1));
    }

    #[test]
    fn line_col_multiline() {
        let input = "abc\ndef";
        assert_eq!(line_col(input, 5, SpanUnit::Byte), (2, 2)); // 'e'
    }

    #[test]
    fn cjk_dsl_error_does_not_panic() {
        // Regression for the char-index vs byte-offset bug: DSL spans are
        // character indices, so byte-slicing them would panic on multi-byte
        // input and abort the wasm module. Must return a clean Err instead.
        let bad = "diagram d {\n  a: \"あいうえお\" @\n}";
        let err = check_impl(bad, "kozue").expect_err("invalid DSL must error, not panic");
        assert!(err.contains("line"), "got: {err}");
    }

    #[test]
    fn dsl_secondary_label_is_surfaced() {
        // Duplicate declaration carries a "first declared here" secondary label
        // that must not be silently dropped (project data-loss principle).
        let dup = "diagram d {\n  a: \"A\"\n  a: \"B\"\n}";
        let err = check_impl(dup, "kozue").expect_err("duplicate id must error");
        assert!(
            err.contains("note at line"),
            "secondary label must be surfaced: {err}"
        );
    }

    #[test]
    fn parse_lang_unknown_returns_err() {
        let result = parse_lang("unknown-lang");
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("unknown language"), "got: {msg}");
    }

    #[test]
    fn svg_impl_deterministic() {
        let svg1 = svg_impl(KOZUE_MINIMAL, "kozue").unwrap();
        let svg2 = svg_impl(KOZUE_MINIMAL, "kozue").unwrap();
        assert_eq!(svg1, svg2, "SVG output must be deterministic");
    }

    #[test]
    fn png_impl_deterministic() {
        let bytes1 = png_impl(KOZUE_MINIMAL, "kozue").unwrap();
        let bytes2 = png_impl(KOZUE_MINIMAL, "kozue").unwrap();
        assert_eq!(
            bytes1, bytes2,
            "PNG output must be deterministic (same bytes)"
        );
    }

    #[test]
    fn svg_impl_unknown_lang_returns_err() {
        let result = svg_impl(KOZUE_MINIMAL, "wat");
        assert!(result.is_err());
    }

    #[test]
    fn png_impl_unknown_lang_returns_err() {
        let result = png_impl(KOZUE_MINIMAL, "unknown");
        assert!(result.is_err());
    }
}
