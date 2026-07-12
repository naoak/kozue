//! Span-to-LSP-position conversion for kozue frontends.
//!
//! Duplicated and adapted from `crates/kozue-wasm/src/lib.rs`; dedup into a
//! shared utility crate is deferred debt (tracked for M6b).

use std::ops::Range;
use tower_lsp::lsp_types::Position;

// ---------------------------------------------------------------------------
// New helpers for M6b (hover / formatting)
// ---------------------------------------------------------------------------

/// Return the LSP [`Position`] that represents the very end of `text`.
///
/// Used to build a whole-document [`TextEdit`] range.
/// - Empty document → `(0, 0)`.
/// - Document ending without a newline → end of last content line.
/// - Document ending with a trailing newline → start of the empty line after it.
pub(crate) fn end_of_document_position(text: &str) -> Position {
    to_position(text, text.len(), SpanUnit::Byte)
}

/// Convert an LSP [`Position`] (UTF-16 line/character) back to a byte offset
/// within `text`. Never panics; clamps at line end / EOF.
///
/// - Walks lines by counting `'\n'` characters.
/// - Within the target line, accumulates `char::len_utf16()` until the
///   requested UTF-16 character column is reached.
pub(crate) fn position_to_byte_offset(text: &str, pos: Position) -> usize {
    // Find the byte offset of the start of the target line.
    let mut line_start = 0;
    let mut current_line = 0u32;
    for (i, b) in text.bytes().enumerate() {
        if current_line == pos.line {
            break;
        }
        if b == b'\n' {
            current_line += 1;
            line_start = i + 1;
        }
    }
    // If we never reached the target line, clamp to EOF.
    if current_line < pos.line {
        return text.len();
    }

    // Walk chars in the line accumulating UTF-16 units until we hit pos.character.
    let line_text = &text[line_start..];
    let mut utf16_col = 0u32;
    for (byte_off, ch) in line_text.char_indices() {
        // Stop at newline (don't cross into next line).
        if ch == '\n' {
            return line_start + byte_off;
        }
        // Return this char's start byte when pos.character falls at its start or
        // *inside* its UTF-16 span. Snapping back to the char start (rather than
        // forward to the next char) makes this a conservative inverse of
        // `to_position`: a cursor between the two surrogates of an astral char
        // resolves to that char, not the following token.
        if utf16_col + ch.len_utf16() as u32 > pos.character {
            return line_start + byte_off;
        }
        utf16_col += ch.len_utf16() as u32;
    }
    // Past end of line / EOF.
    line_start + line_text.len()
}

/// The unit a frontend's diagnostic spans are measured in.
///
/// `kozue-dsl` (chumsky) emits character indices; `kozue-mermaid` and
/// `kozue-plantuml` emit byte offsets. We must convert correctly before slicing.
#[derive(Debug, Clone, Copy)]
pub(crate) enum SpanUnit {
    /// chumsky character index (one per Unicode scalar value).
    Char,
    /// Byte offset (as produced by hand-written scanners).
    Byte,
}

/// Convert a span index in `unit` to a guaranteed-valid UTF-8 char boundary
/// byte offset within `input`. Never panics, even on multi-byte/CJK/BOM input.
fn to_byte_offset(input: &str, index: usize, unit: SpanUnit) -> usize {
    match unit {
        SpanUnit::Byte => {
            // Clamp into range, then snap down to nearest char boundary.
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

/// Compute an LSP [`Position`] for a span `index` (in `unit`) within `input`.
///
/// - Line is 0-based (count of '\n' before the byte offset).
/// - Character is a **UTF-16 code unit** offset within the line, as required
///   by the default LSP position encoding (UTF-16). Note that astral-plane
///   codepoints such as emoji (e.g. U+1F389 🎉) encode as **two** UTF-16 code
///   units, so `character` can exceed the char count for such lines.
pub(crate) fn to_position(input: &str, index: usize, unit: SpanUnit) -> Position {
    let byte = to_byte_offset(input, index, unit);
    let prefix = &input[..byte];
    let line = prefix.bytes().filter(|&b| b == b'\n').count() as u32;
    let line_start = prefix.rfind('\n').map(|p| p + 1).unwrap_or(0);
    // LSP character is UTF-16 code units, not chars and not bytes.
    let character = input[line_start..byte]
        .chars()
        .map(|c| c.len_utf16() as u32)
        .sum();
    Position { line, character }
}

/// Convert a `Range<usize>` span (in `unit`) to an LSP [`Range`].
pub(crate) fn to_range(
    input: &str,
    span: &Range<usize>,
    unit: SpanUnit,
) -> tower_lsp::lsp_types::Range {
    tower_lsp::lsp_types::Range {
        start: to_position(input, span.start, unit),
        end: to_position(input, span.end, unit),
    }
}

// ---------------------------------------------------------------------------
// Tests — no async, no tower-lsp: just pure string/position arithmetic.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    // ---- ASCII ----

    #[test]
    fn ascii_start_of_file() {
        let pos = to_position("hello", 0, SpanUnit::Byte);
        assert_eq!(
            pos,
            Position {
                line: 0,
                character: 0
            }
        );
    }

    #[test]
    fn ascii_mid_line() {
        // 'e' is byte 1 / char 1.
        let pos = to_position("hello", 1, SpanUnit::Byte);
        assert_eq!(
            pos,
            Position {
                line: 0,
                character: 1
            }
        );
    }

    #[test]
    fn ascii_eof() {
        let input = "abc";
        // Byte 3 is one past the end → clamped to len → position at end.
        let pos = to_position(input, 3, SpanUnit::Byte);
        assert_eq!(
            pos,
            Position {
                line: 0,
                character: 3
            }
        );
    }

    #[test]
    fn ascii_empty_input() {
        let pos = to_position("", 0, SpanUnit::Byte);
        assert_eq!(
            pos,
            Position {
                line: 0,
                character: 0
            }
        );
    }

    // ---- Multi-line ----

    #[test]
    fn multiline_second_line_start() {
        let input = "abc\ndef";
        // byte 4 = 'd'
        let pos = to_position(input, 4, SpanUnit::Byte);
        assert_eq!(
            pos,
            Position {
                line: 1,
                character: 0
            }
        );
    }

    #[test]
    fn multiline_second_line_mid() {
        let input = "abc\ndef";
        // byte 5 = 'e'
        let pos = to_position(input, 5, SpanUnit::Byte);
        assert_eq!(
            pos,
            Position {
                line: 1,
                character: 1
            }
        );
    }

    #[test]
    fn multiline_newline_itself() {
        let input = "abc\ndef";
        // byte 3 = '\n'
        let pos = to_position(input, 3, SpanUnit::Byte);
        assert_eq!(
            pos,
            Position {
                line: 0,
                character: 3
            }
        );
    }

    // ---- CJK (3-byte / 1-char / 1-UTF16-unit) ----

    #[test]
    fn cjk_byte_unit_start() {
        // "あ" = 3 bytes, 1 char, 1 UTF-16 unit.
        let input = "あx";
        let pos = to_position(input, 0, SpanUnit::Byte);
        assert_eq!(
            pos,
            Position {
                line: 0,
                character: 0
            }
        );
    }

    #[test]
    fn cjk_byte_unit_after() {
        // byte 3 → after "あ" = character offset 1.
        let pos = to_position("あx", 3, SpanUnit::Byte);
        assert_eq!(
            pos,
            Position {
                line: 0,
                character: 1
            }
        );
    }

    #[test]
    fn cjk_byte_unit_inside_snaps() {
        // byte 1 lands inside "あ" (3 bytes); must snap without panic.
        let pos = to_position("あx", 1, SpanUnit::Byte);
        assert_eq!(
            pos,
            Position {
                line: 0,
                character: 0
            }
        );
    }

    #[test]
    fn cjk_char_unit() {
        // char index 0 → "あ"; char index 1 → "x".
        let pos = to_position("あx", 1, SpanUnit::Char);
        assert_eq!(
            pos,
            Position {
                line: 0,
                character: 1
            }
        );
    }

    #[test]
    fn cjk_multiline_char_unit() {
        let input = "あ\nいx";
        // char 2 → 'い' on line 1, char offset 0.
        let pos = to_position(input, 2, SpanUnit::Char);
        assert_eq!(
            pos,
            Position {
                line: 1,
                character: 0
            }
        );
    }

    // ---- Astral/emoji (4 bytes / 1 char / 2 UTF-16 units) ----

    #[test]
    fn emoji_byte_unit_after() {
        // "🎉" = 4 bytes, 1 char, 2 UTF-16 units.
        // byte 4 → after "🎉" = UTF-16 character offset 2.
        let pos = to_position("🎉x", 4, SpanUnit::Byte);
        assert_eq!(
            pos,
            Position {
                line: 0,
                character: 2
            }
        );
    }

    #[test]
    fn emoji_char_unit_after() {
        // char 1 → after "🎉" = UTF-16 character offset 2.
        let pos = to_position("🎉x", 1, SpanUnit::Char);
        assert_eq!(
            pos,
            Position {
                line: 0,
                character: 2
            }
        );
    }

    #[test]
    fn emoji_byte_unit_inside_snaps() {
        // Any byte 1..=3 inside "🎉" must snap to byte 0 → position 0,0.
        for b in 1..=3usize {
            let pos = to_position("🎉x", b, SpanUnit::Byte);
            assert_eq!(
                pos,
                Position {
                    line: 0,
                    character: 0
                },
                "byte {b} should snap"
            );
        }
    }

    #[test]
    fn emoji_two_in_row() {
        // "🎉🎉" → char 2 = byte 8; UTF-16 character = 4.
        let pos = to_position("🎉🎉x", 2, SpanUnit::Char);
        assert_eq!(
            pos,
            Position {
                line: 0,
                character: 4
            }
        );
    }

    // ---- Leading BOM ----

    #[test]
    fn leading_bom_byte_unit() {
        // BOM = U+FEFF = 3 bytes in UTF-8, 1 UTF-16 unit.
        let input = "\u{feff}abc";
        // byte 3 → 'a' = UTF-16 character 1 (BOM is 1 UTF-16 unit).
        let pos = to_position(input, 3, SpanUnit::Byte);
        assert_eq!(
            pos,
            Position {
                line: 0,
                character: 1
            }
        );
    }

    #[test]
    fn leading_bom_char_unit() {
        let input = "\u{feff}abc";
        // char 1 → 'a' = UTF-16 character 1.
        let pos = to_position(input, 1, SpanUnit::Char);
        assert_eq!(
            pos,
            Position {
                line: 0,
                character: 1
            }
        );
    }

    // ---- Span at EOF ----

    #[test]
    fn span_at_eof_byte() {
        let input = "abc";
        let range = to_range(input, &(3..3), SpanUnit::Byte);
        assert_eq!(
            range.start,
            Position {
                line: 0,
                character: 3
            }
        );
        assert_eq!(
            range.end,
            Position {
                line: 0,
                character: 3
            }
        );
    }

    #[test]
    fn span_at_eof_char() {
        let input = "abc";
        let range = to_range(input, &(3..3), SpanUnit::Char);
        assert_eq!(
            range.start,
            Position {
                line: 0,
                character: 3
            }
        );
        assert_eq!(
            range.end,
            Position {
                line: 0,
                character: 3
            }
        );
    }

    // ---- to_range ----

    #[test]
    fn range_basic() {
        let input = "hello world";
        // span 6..11 = "world"
        let r = to_range(input, &(6..11), SpanUnit::Byte);
        assert_eq!(
            r.start,
            Position {
                line: 0,
                character: 6
            }
        );
        assert_eq!(
            r.end,
            Position {
                line: 0,
                character: 11
            }
        );
    }

    #[test]
    fn range_multiline() {
        let input = "abc\ndefgh";
        // span 4..7 = "def" on line 1
        let r = to_range(input, &(4..7), SpanUnit::Byte);
        assert_eq!(
            r.start,
            Position {
                line: 1,
                character: 0
            }
        );
        assert_eq!(
            r.end,
            Position {
                line: 1,
                character: 3
            }
        );
    }

    // ---- SpanUnit::Char for DSL path ----

    #[test]
    fn char_unit_ascii_identical_to_byte() {
        // ASCII: char index == byte offset.
        let input = "hello";
        for i in 0..=5 {
            assert_eq!(
                to_position(input, i, SpanUnit::Char),
                to_position(input, i, SpanUnit::Byte),
                "index {i}"
            );
        }
    }

    #[test]
    fn char_unit_beyond_end_clamps() {
        // char index beyond end → EOF position.
        let input = "ab";
        let pos = to_position(input, 100, SpanUnit::Char);
        assert_eq!(
            pos,
            Position {
                line: 0,
                character: 2
            }
        );
    }

    #[test]
    fn byte_unit_beyond_end_clamps() {
        let input = "ab";
        let pos = to_position(input, 100, SpanUnit::Byte);
        assert_eq!(
            pos,
            Position {
                line: 0,
                character: 2
            }
        );
    }

    // ---- end_of_document_position ----

    #[test]
    fn end_of_doc_empty() {
        assert_eq!(
            end_of_document_position(""),
            Position {
                line: 0,
                character: 0
            }
        );
    }

    #[test]
    fn end_of_doc_trailing_newline() {
        // "abc\n" → end is position (1, 0): the empty line after the '\n'.
        assert_eq!(
            end_of_document_position("abc\n"),
            Position {
                line: 1,
                character: 0
            }
        );
    }

    #[test]
    fn end_of_doc_no_trailing_newline() {
        // "abc" → end is (0, 3).
        assert_eq!(
            end_of_document_position("abc"),
            Position {
                line: 0,
                character: 3
            }
        );
    }

    #[test]
    fn end_of_doc_multiline() {
        // "a\nb\nc" → end is (2, 1).
        assert_eq!(
            end_of_document_position("a\nb\nc"),
            Position {
                line: 2,
                character: 1
            }
        );
    }

    #[test]
    fn end_of_doc_cjk() {
        // "あ\nい" → CJK are 1 UTF-16 unit each → end is (1, 1).
        assert_eq!(
            end_of_document_position("あ\nい"),
            Position {
                line: 1,
                character: 1
            }
        );
    }

    #[test]
    fn end_of_doc_emoji() {
        // "🎉" → 2 UTF-16 units → end is (0, 2).
        assert_eq!(
            end_of_document_position("🎉"),
            Position {
                line: 0,
                character: 2
            }
        );
    }

    // ---- position_to_byte_offset ----

    #[test]
    fn p2b_ascii_basic() {
        let text = "hello";
        // position (0, 3) → byte 3 ('l').
        assert_eq!(
            position_to_byte_offset(
                text,
                Position {
                    line: 0,
                    character: 3
                }
            ),
            3
        );
    }

    #[test]
    fn p2b_ascii_start() {
        let text = "hello";
        assert_eq!(
            position_to_byte_offset(
                text,
                Position {
                    line: 0,
                    character: 0
                }
            ),
            0
        );
    }

    #[test]
    fn p2b_empty_doc() {
        let text = "";
        assert_eq!(
            position_to_byte_offset(
                text,
                Position {
                    line: 0,
                    character: 0
                }
            ),
            0
        );
    }

    #[test]
    fn p2b_multiline() {
        let text = "abc\ndef";
        // line 1, char 2 → 'd'=byte4, 'e'=byte5 → byte 6 ('f').
        assert_eq!(
            position_to_byte_offset(
                text,
                Position {
                    line: 1,
                    character: 2
                }
            ),
            6
        );
    }

    #[test]
    fn p2b_cjk() {
        // "あx": "あ" = 3 bytes, 1 UTF-16 unit. Position (0,1) → byte 3.
        let text = "あx";
        assert_eq!(
            position_to_byte_offset(
                text,
                Position {
                    line: 0,
                    character: 1
                }
            ),
            3
        );
    }

    #[test]
    fn p2b_emoji_astral() {
        // "🎉x": "🎉" = 4 bytes, 2 UTF-16 units. Position (0,2) → byte 4.
        let text = "🎉x";
        assert_eq!(
            position_to_byte_offset(
                text,
                Position {
                    line: 0,
                    character: 2
                }
            ),
            4
        );
    }

    #[test]
    fn p2b_emoji_midpoint_snaps_back() {
        // "🎉x": a position landing between the two surrogates of "🎉" (col 1)
        // must snap BACK to the emoji's start (byte 0), not forward to 'x'.
        let text = "🎉x";
        assert_eq!(
            position_to_byte_offset(
                text,
                Position {
                    line: 0,
                    character: 1
                }
            ),
            0
        );
    }

    #[test]
    fn p2b_past_line_end_clamps() {
        // Position past end of line clamps to the line's end byte.
        let text = "abc\ndef";
        // line 0 ends at byte 3 (the '\n'); col 100 → byte 3.
        assert_eq!(
            position_to_byte_offset(
                text,
                Position {
                    line: 0,
                    character: 100
                }
            ),
            3
        );
    }

    #[test]
    fn p2b_past_eof_clamps() {
        // Line past end of document → EOF.
        let text = "abc";
        assert_eq!(
            position_to_byte_offset(
                text,
                Position {
                    line: 99,
                    character: 0
                }
            ),
            3
        );
    }

    #[test]
    fn p2b_roundtrip_ascii() {
        // to_position(text, position_to_byte_offset(text, p), Byte) == p
        let text = "hello\nworld";
        for (line, character) in [(0, 0), (0, 3), (0, 5), (1, 0), (1, 5)] {
            let p = Position { line, character };
            let byte = position_to_byte_offset(text, p);
            assert_eq!(
                to_position(text, byte, SpanUnit::Byte),
                p,
                "round-trip failed at ({line},{character})"
            );
        }
    }

    #[test]
    fn p2b_roundtrip_emoji() {
        // Emoji: each 🎉 takes 2 UTF-16 units, 4 bytes.
        let text = "🎉🎉\nabc";
        for (line, character) in [(0, 0), (0, 2), (0, 4), (1, 0), (1, 3)] {
            let p = Position { line, character };
            let byte = position_to_byte_offset(text, p);
            assert_eq!(
                to_position(text, byte, SpanUnit::Byte),
                p,
                "round-trip failed at ({line},{character})"
            );
        }
    }
}
