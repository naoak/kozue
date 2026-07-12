//! PlantUML-syntax frontend for kozue.
//!
//! Parses a subset of PlantUML sequence diagrams into the
//! [`kozue_ir::Diagram`] semantic IR used by the native kozue DSL. Layout and
//! rendering are handled by the existing kozue pipeline unchanged.
//!
//! # Scope (M4)
//!
//! M4 targets **PlantUML sequence diagrams only** — preprocessor-free. Because
//! PlantUML does not declare the diagram type in a dedicated header line (the
//! same `A -> B` syntax is valid in sequence, component, and other diagrams),
//! auto-guessing the diagram kind would violate the never-silently-misparse
//! principle. Therefore component, state, class, activity, and all other
//! non-sequence diagram types are explicitly **out of scope** and produce clear
//! "unsupported" errors.
//!
//! # Supported syntax
//!
//! ```text
//! @startuml
//! ' single-line comment
//! /' block comment '/
//! participant Alice
//! participant Bob as B
//! participant "長い名前" as LN
//! actor User
//! boundary SomeService
//!
//! Alice -> B : こんにちは
//! B --> Alice : 返事
//! Alice ->> Alice : 自己メッセージ
//! @enduml
//! ```
//!
//! # Compatibility notes
//!
//! - `->>`/`-->>` open/thin arrowheads are mapped to `ArrowType::Triangle`
//!   (same behaviour as `->` / `-->`). The arrowhead distinction is not rendered.
//! - Icon-variant keywords (`boundary`, `control`, `entity`, `database`,
//!   `collections`, `queue`) are parsed and mapped to `Participant`; the icon
//!   is not rendered.
//! - Unsupported features (notes, alt/loop/opt blocks, activate/deactivate,
//!   skinparam, preprocessor directives, non-sequence @start<type>, etc.) are
//!   reported as positioned "unsupported" errors rather than crashing or
//!   silently ignoring.
//! - `end` that closes an unsupported block is silently skipped (same as
//!   kozue-mermaid behaviour).

pub mod features;

use std::ops::Range;

use ariadne::{Label, Report, ReportKind, Source};
use kozue_ir::{
    ArrowType, Diagram, LineStyle, Message, Participant, SequenceDiagram, SequenceItem,
};

/// A user-facing parse/semantic error with a byte-offset span.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub message: String,
    pub span: Range<usize>,
}

impl Diagnostic {
    fn new(message: impl Into<String>, span: Range<usize>) -> Self {
        Diagnostic {
            message: message.into(),
            span,
        }
    }
}

/// Parse PlantUML source text into a semantic [`Diagram`].
///
/// Returns `Ok(diagram)` on success, or `Err(diagnostics)` where diagnostics
/// is a non-empty list of errors (all errors from the whole source are
/// collected before returning, following the same convention as `kozue-dsl`).
pub fn parse(source: &str) -> Result<Diagram, Vec<Diagnostic>> {
    let mut errors: Vec<Diagnostic> = Vec::new();

    // Strip UTF-8 BOM if present.
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);

    // Mask all comments (block `/' '/` and line `'`) with spaces, preserving byte
    // length and newlines so diagnostic spans map back to the original source.
    // Quote-aware: `'` / `/'` / `'/` inside a `"..."` string are literal.
    let (masked, unterminated_block) = mask_comments(source);

    // An unterminated block comment masks everything to EOF; surface it explicitly
    // rather than silently dropping the swallowed content.
    if let Some(span) = unterminated_block {
        errors.push(Diagnostic::new(
            "unterminated block comment: `/'` was never closed with `'/`",
            span,
        ));
    }

    // Tokenise into logical lines (drop blank lines), keeping original byte offsets.
    let lines: Vec<(usize, String)> = logical_lines(&masked);

    if lines.is_empty() {
        errors.push(Diagnostic::new(
            "empty diagram: expected `@startuml` header",
            0..source.len().max(1),
        ));
        return Err(errors);
    }

    // First non-comment logical line MUST be @startuml (optionally @startuml SomeName).
    let (header_offset, ref header_line) = lines[0];
    let header_trimmed = header_line.trim();

    // Check for @start<other> variants first.
    if let Some(rest) = header_trimmed.strip_prefix("@start") {
        let kind = rest.split_whitespace().next().unwrap_or(rest);
        if !kind.eq_ignore_ascii_case("uml") {
            errors.push(Diagnostic::new(
                format!(
                    "unsupported: @start{} diagram type; kozue-plantuml only supports @startuml (sequence diagrams)",
                    kind
                ),
                header_offset..header_offset + header_line.len(),
            ));
            return Err(errors);
        }
        // It is @startuml. Any trailing text is PlantUML's optional diagram *name*,
        // which is purely a document title with no bearing on the sequence's
        // semantics or layout — discarding it loses no diagram content, so no
        // diagnostic is warranted (documented in features.rs).
    } else {
        errors.push(Diagnostic::new(
            format!(
                "missing `@startuml`: first non-comment line must be `@startuml`, got `{}`",
                header_trimmed.chars().take(40).collect::<String>()
            ),
            header_offset..header_offset + header_line.len(),
        ));
        return Err(errors);
    }

    // Last non-comment logical line MUST be @enduml.
    let (enduml_offset, ref enduml_line) = lines[lines.len() - 1];
    let enduml_trimmed = enduml_line.trim();
    let has_enduml = enduml_trimmed.eq_ignore_ascii_case("@enduml");
    if !has_enduml {
        errors.push(Diagnostic::new(
            "missing `@enduml`: last non-comment line must be `@enduml`",
            enduml_offset..enduml_offset + enduml_line.len(),
        ));
        // Keep going to collect body errors, but use a sentinel empty body.
    }

    // Body is everything between @startuml and @enduml.
    let body_end = if has_enduml {
        lines.len() - 1
    } else {
        lines.len()
    };
    let body = &lines[1..body_end];

    parse_sequence_body(body, source, &mut errors);

    if errors.is_empty() {
        // Re-parse cleanly to produce the diagram (errors already empty).
        Ok(parse_sequence_clean(body))
    } else {
        Err(errors)
    }
}

/// Render diagnostics to stderr using ariadne (matches the kozue-dsl convention).
pub fn report_errors(filename: &str, src: &str, errors: &[Diagnostic]) {
    for err in errors {
        let span = err.span.clone();
        Report::build(ReportKind::Error, filename, span.start)
            .with_message(&err.message)
            .with_label(Label::new((filename, span)).with_message(&err.message))
            .finish()
            .eprint((filename, Source::from(src)))
            .ok();
    }
}

// ---------------------------------------------------------------------------
// Sequence body parser (error-collecting pass)
// ---------------------------------------------------------------------------

fn parse_sequence_body(lines: &[(usize, String)], _source: &str, errors: &mut Vec<Diagnostic>) {
    // Ids introduced by explicit `participant`/`actor`/… declarations, to detect
    // duplicate declarations (matching the kozue-dsl frontend, which rejects them).
    let mut declared_ids: Vec<String> = Vec::new();
    for (offset, line) in lines {
        let trimmed = line.trim();
        let line_end = offset + line.len();
        let span = *offset..line_end;

        if trimmed.is_empty() {
            continue;
        }

        // Preprocessor lines: any line whose first non-space char is `!`.
        if trimmed.starts_with('!') {
            errors.push(Diagnostic::new(
                "unsupported: PlantUML preprocessor directives (`!...`) are not supported; kozue targets a preprocessor-free subset",
                span,
            ));
            continue;
        }

        // `== divider ==` lines.
        if trimmed.starts_with("==") {
            errors.push(Diagnostic::new(
                "unsupported: == dividers are not supported",
                span,
            ));
            continue;
        }

        // `...` or `||` delay lines.
        if trimmed == "..." || trimmed.starts_with("...") || trimmed.starts_with("||") {
            errors.push(Diagnostic::new(
                "unsupported: delay markers (`...` / `||`) are not supported",
                span,
            ));
            continue;
        }

        // Participant / actor / icon-variant keyword declarations.
        if let Some((id, _)) = try_parse_participant_decl(trimmed) {
            if declared_ids.iter().any(|d| d == &id) {
                errors.push(Diagnostic::new(
                    format!("duplicate participant id `{id}`"),
                    span.clone(),
                ));
            } else {
                declared_ids.push(id);
            }
            continue;
        }

        // Message arrow lines.
        if let Some(result) = try_parse_plantuml_message(trimmed) {
            match result {
                Ok(_) => {
                    // Valid — no error.
                }
                Err(msg) => {
                    errors.push(Diagnostic::new(msg, span));
                }
            }
            continue;
        }

        // `end` silently skipped (closes unsupported alt/loop/opt/group blocks).
        if trimmed == "end" {
            continue;
        }

        // Unsupported keywords — word-boundary matched so `participant` isn't caught by `par`.
        let unsupported_kw: &[(&str, &str)] = &[
            (
                "note",
                "unsupported: note (kozue does not support this yet)",
            ),
            (
                "hnote",
                "unsupported: hnote (kozue does not support this yet)",
            ),
            (
                "rnote",
                "unsupported: rnote (kozue does not support this yet)",
            ),
            ("alt", "unsupported: alt (kozue does not support this yet)"),
            (
                "else",
                "unsupported: else (kozue does not support this yet)",
            ),
            ("opt", "unsupported: opt (kozue does not support this yet)"),
            (
                "loop",
                "unsupported: loop (kozue does not support this yet)",
            ),
            ("par", "unsupported: par (kozue does not support this yet)"),
            (
                "break",
                "unsupported: break (kozue does not support this yet)",
            ),
            (
                "critical",
                "unsupported: critical (kozue does not support this yet)",
            ),
            (
                "group",
                "unsupported: group (kozue does not support this yet)",
            ),
            (
                "activate",
                "unsupported: activate (kozue does not support this yet)",
            ),
            (
                "deactivate",
                "unsupported: deactivate (kozue does not support this yet)",
            ),
            (
                "destroy",
                "unsupported: destroy (kozue does not support this yet)",
            ),
            (
                "create",
                "unsupported: create (kozue does not support this yet)",
            ),
            (
                "return",
                "unsupported: return (kozue does not support this yet)",
            ),
            (
                "autonumber",
                "unsupported: autonumber (kozue does not support this yet)",
            ),
            (
                "title",
                "unsupported: title (kozue does not support this yet)",
            ),
            (
                "header",
                "unsupported: header (kozue does not support this yet)",
            ),
            (
                "footer",
                "unsupported: footer (kozue does not support this yet)",
            ),
            (
                "newpage",
                "unsupported: newpage (kozue does not support this yet)",
            ),
            ("box", "unsupported: box (kozue does not support this yet)"),
            ("ref", "unsupported: ref (kozue does not support this yet)"),
            (
                "hide",
                "unsupported: hide (kozue does not support this yet)",
            ),
            (
                "show",
                "unsupported: show (kozue does not support this yet)",
            ),
            (
                "skinparam",
                "unsupported: skinparam (kozue does not support this yet)",
            ),
        ];

        let mut found_unsupported = false;
        for &(kw, msg) in unsupported_kw {
            // Word-boundary: keyword must be followed by whitespace or end-of-string.
            if trimmed == kw
                || (trimmed.starts_with(kw)
                    && trimmed[kw.len()..].starts_with(|c: char| c.is_ascii_whitespace()))
            {
                errors.push(Diagnostic::new(msg, span.clone()));
                found_unsupported = true;
                break;
            }
        }
        if found_unsupported {
            continue;
        }

        // Unrecognised line.
        errors.push(Diagnostic::new(
            format!(
                "syntax error: unrecognised statement `{}`",
                trimmed.chars().take(40).collect::<String>()
            ),
            span,
        ));
    }
}

/// Clean parse pass — only called when parse_sequence_body found no errors.
fn parse_sequence_clean(lines: &[(usize, String)]) -> Diagram {
    let mut seq = SequenceDiagram::new();

    let ensure_participant = |seq: &mut SequenceDiagram, id: &str, label: Option<&str>| {
        if !seq.participants.contains_key(id) {
            let lbl = label.unwrap_or(id).to_string();
            seq.participants
                .insert(id.to_string(), Participant::new(id.to_string(), lbl));
        }
    };

    for (_offset, line) in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed == "end" {
            continue;
        }

        if let Some((id, label)) = try_parse_participant_decl(trimmed) {
            ensure_participant(&mut seq, &id, label.as_deref());
            continue;
        }

        if let Some(Ok((from, to, label, line_style, arrow))) = try_parse_plantuml_message(trimmed)
        {
            ensure_participant(&mut seq, &from, None);
            ensure_participant(&mut seq, &to, None);
            seq.items.push(SequenceItem::Message(Message::new(
                from, to, label, line_style, arrow,
            )));
        }
    }

    Diagram::Sequence(seq)
}

// ---------------------------------------------------------------------------
// Comment masking (block `/' '/` and line `'`)
// ---------------------------------------------------------------------------

/// Mask every comment in `source` with spaces, in a single quote-aware,
/// byte-length-preserving pass.
///
/// Block comments (`/' ... '/`, possibly multi-line) and line comments (`'`
/// to end-of-line) are replaced by spaces; newlines are kept in place. Every
/// input byte maps to exactly one output byte, so byte offsets — and therefore
/// diagnostic spans — line up with the original source.
///
/// Quote-aware: a `'`, `/'`, or `'/` inside a `"..."` string is literal text
/// and never starts/ends a comment; a `"` inside a comment is ignored. This is
/// what prevents a `/'` inside a line comment or quoted display name from
/// opening a phantom block comment and silently swallowing later input.
///
/// Returns the masked string and, if the source ended while still inside a
/// block comment, the span of the unterminated `/'` opener so the caller can
/// emit a diagnostic instead of silently swallowing everything after it.
fn mask_comments(source: &str) -> (String, Option<Range<usize>>) {
    #[derive(PartialEq)]
    enum Mode {
        Normal,
        Quote,
        Line,
        Block,
    }

    let mut out = String::with_capacity(source.len());
    let mut mode = Mode::Normal;
    // Byte offset of the `/'` that opened the block comment we are currently in.
    let mut block_open: Option<usize> = None;
    let mut chars = source.char_indices().peekable();

    // Push `n` spaces (used to mask an interior char while preserving byte length).
    let push_spaces = |out: &mut String, n: usize| {
        for _ in 0..n {
            out.push(' ');
        }
    };

    while let Some((i, c)) = chars.next() {
        match mode {
            Mode::Normal => {
                if c == '"' {
                    out.push('"');
                    mode = Mode::Quote;
                } else if c == '/' && matches!(chars.peek(), Some(&(_, '\''))) {
                    chars.next(); // consume the '\'' of the `/'` opener (both ASCII)
                    out.push(' ');
                    out.push(' ');
                    mode = Mode::Block;
                    block_open = Some(i);
                } else if c == '\'' {
                    out.push(' ');
                    mode = Mode::Line;
                } else {
                    out.push(c);
                }
            }
            Mode::Quote => {
                out.push(c);
                // PlantUML string literals are single-line; recover at newline so an
                // unterminated quote can't swallow the rest of the file.
                if c == '"' || c == '\n' {
                    mode = Mode::Normal;
                }
            }
            Mode::Line => {
                if c == '\n' {
                    out.push('\n');
                    mode = Mode::Normal;
                } else {
                    push_spaces(&mut out, c.len_utf8());
                }
            }
            Mode::Block => {
                if c == '\'' && matches!(chars.peek(), Some(&(_, '/'))) {
                    chars.next(); // consume the '/' of the `'/` closer (both ASCII)
                    out.push(' ');
                    out.push(' ');
                    mode = Mode::Normal;
                    block_open = None;
                } else if c == '\n' {
                    out.push('\n');
                } else {
                    push_spaces(&mut out, c.len_utf8());
                }
            }
        }
    }

    // If we ended inside a block comment, report the opener so nothing is dropped
    // without a diagnostic.
    let unterminated = if mode == Mode::Block {
        block_open.map(|s| s..source.len())
    } else {
        None
    };

    (out, unterminated)
}

/// Collect non-blank lines from the comment-masked source, with their byte
/// offsets into the original source (byte length is preserved by masking).
fn logical_lines(masked: &str) -> Vec<(usize, String)> {
    let mut result = Vec::new();
    let mut offset = 0usize;

    for raw in masked.split('\n') {
        if !raw.trim().is_empty() {
            result.push((offset, raw.to_string()));
        }
        offset += raw.len() + 1; // +1 for the '\n' consumed by split
    }

    result
}

// ---------------------------------------------------------------------------
// Participant declaration parser
// ---------------------------------------------------------------------------

/// Keywords that introduce participant declarations (in addition to `participant` and `actor`).
const PARTICIPANT_KEYWORDS: &[&str] = &[
    "participant",
    "actor",
    "boundary",
    "control",
    "entity",
    "database",
    "collections",
    "queue",
];

/// Try to parse a participant/actor/icon-variant declaration.
///
/// Returns `Some((id, Option<label>))` on success, `None` if not a participant line.
/// - `participant Name` → id="Name", label=None (label defaults to id)
/// - `participant Name as Alias` → id="Alias", label=Some("Name")
/// - `participant "Quoted Name" as Alias` → id="Alias", label=Some("Quoted Name")
fn try_parse_participant_decl(line: &str) -> Option<(String, Option<String>)> {
    let (kw, rest) = split_keyword(line)?;
    if !PARTICIPANT_KEYWORDS
        .iter()
        .any(|&k| k.eq_ignore_ascii_case(kw))
    {
        return None;
    }
    let rest = rest.trim();

    // Check for quoted display name: `"Quoted Name" as Alias`.
    if let Some(after_open_quote) = rest.strip_prefix('"') {
        // Find the closing quote.
        let end_quote = after_open_quote.find('"')?;
        let display_name = after_open_quote[..end_quote].to_string();
        let after_quote = after_open_quote[end_quote + 1..].trim(); // skip closing "
        if after_quote.is_empty() {
            // No `as` — use the quoted name as both id and label.
            return Some((display_name.clone(), Some(display_name)));
        }
        if let Some(alias) = strip_keyword_boundary_ci(after_quote, "as") {
            let alias = alias.trim().to_string();
            // Alias must be a single bare token; anything else is malformed.
            if !is_single_token(&alias) {
                return None;
            }
            return Some((alias, Some(display_name)));
        }
        // Trailing tokens that are not an `as` clause — reject rather than
        // silently discard them.
        return None;
    }

    // Unquoted name: `Name` or `Name as Alias`.
    if let Some(as_idx) = find_as_boundary(rest) {
        // `Name as Alias`
        let name = rest[..as_idx].trim().to_string();
        let alias = rest[as_idx + 4..].trim().to_string(); // " as " is 4 chars
                                                           // Both parts must be single bare tokens (unquoted names cannot contain
                                                           // whitespace); otherwise this is a malformed declaration, not a phantom
                                                           // participant.
        if !is_single_token(&name) || !is_single_token(&alias) {
            return None;
        }
        Some((alias, Some(name)))
    } else {
        // `Name` — id and label are both the name. A trailing bare `as` or an
        // interior space (e.g. `participant Foo as`, `participant Foo Bar`)
        // makes this malformed; reject so the caller reports an error.
        let name = rest.to_string();
        if !is_single_token(&name) {
            return None;
        }
        Some((name, None))
    }
}

/// A bare (unquoted) PlantUML name/alias: non-empty and free of whitespace.
fn is_single_token(s: &str) -> bool {
    !s.is_empty() && !s.chars().any(|c| c.is_whitespace())
}

/// Split off the first whitespace-delimited keyword from `line`.
/// Returns `(keyword, rest_after_whitespace)` or None if line is empty.
fn split_keyword(line: &str) -> Option<(&str, &str)> {
    let line = line.trim_start();
    if line.is_empty() {
        return None;
    }
    let end = line
        .char_indices()
        .find(|&(_, c)| c.is_ascii_whitespace())
        .map(|(i, _)| i)
        .unwrap_or(line.len());
    let kw = &line[..end];
    let rest = line[end..].trim_start();
    Some((kw, rest))
}

/// Strip `as ` keyword prefix with word-boundary check (case-insensitive).
/// Returns the text after `as` if matched, None otherwise.
fn strip_keyword_boundary_ci<'a>(s: &'a str, keyword: &str) -> Option<&'a str> {
    if s.len() >= keyword.len()
        && s[..keyword.len()].eq_ignore_ascii_case(keyword)
        && (s.len() == keyword.len()
            || s[keyword.len()..].starts_with(|c: char| c.is_ascii_whitespace()))
    {
        Some(&s[keyword.len()..])
    } else {
        None
    }
}

/// Find ` as ` in `s` with word-boundary semantics, returning the index of ` as `.
fn find_as_boundary(s: &str) -> Option<usize> {
    // Look for " as " (with leading space) — this ensures word boundary.
    let needle = " as ";
    let idx = s.find(needle)?;
    Some(idx)
}

// ---------------------------------------------------------------------------
// Message parser
// ---------------------------------------------------------------------------

/// (from, to, label, line_style, arrow)
type SeqMsgResult = (String, String, Option<String>, LineStyle, ArrowType);

/// Try to parse a PlantUML sequence message line.
///
/// Arrow forms and their mapping:
/// | PlantUML | LineStyle | ArrowType |
/// |----------|-----------|-----------|
/// | `->` | Solid | Triangle |
/// | `-->` | Dashed | Triangle |
/// | `->>` | Solid | Triangle (partial: thin arrow mapped to Triangle) |
/// | `-->>` | Dashed | Triangle (partial: thin arrow mapped to Triangle) |
///
/// Format: `From ARROW To : label` or `From ARROW To`
/// Participants may be bare identifiers.
///
/// Returns `None` if the line does not look like a message.
/// Returns `Some(Err(msg))` if it looks like a message but has a parse error.
/// Returns `Some(Ok(...))` on success.
fn try_parse_plantuml_message(line: &str) -> Option<Result<SeqMsgResult, String>> {
    // Check for unsupported colored arrows: -[#color]>
    if line.contains("-[") {
        return Some(Err(
            "unsupported: colored arrows (`-[#color]>`) are not supported".to_string(),
        ));
    }

    // Arrow forms: longest first to avoid prefix ambiguity.
    // Also check for unsupported ->x and ->o (lost/found messages).
    let arrow_forms: &[(&str, LineStyle, ArrowType)] = &[
        ("-->>", LineStyle::Dashed, ArrowType::Triangle),
        ("-->", LineStyle::Dashed, ArrowType::Triangle),
        ("->>", LineStyle::Solid, ArrowType::Triangle),
        ("->", LineStyle::Solid, ArrowType::Triangle),
    ];

    // Check for ->x or ->o (lost/found) before the normal arrow forms.
    if let Some(idx) = line.find("->x").or_else(|| line.find("->o")) {
        // Verify there's something before it that could be a participant.
        let from_part = line[..idx].trim();
        if !from_part.is_empty() && is_valid_participant_id(from_part) {
            return Some(Err(
                "unsupported: lost/found messages (`->x` / `->o`) are not supported".to_string(),
            ));
        }
    }

    for &(arrow_str, line_style, arrow) in arrow_forms {
        if let Some(idx) = line.find(arrow_str) {
            let from_part = line[..idx].trim();
            let after = &line[idx + arrow_str.len()..];

            // from_part must be a valid participant id.
            if from_part.is_empty() || !is_valid_participant_id(from_part) {
                continue;
            }
            let from = from_part.to_string();

            // Parse `To : label` or just `To`.
            let after = after.trim_start();
            let (to, label) = if let Some(colon_idx) = after.find(':') {
                let to_part = after[..colon_idx].trim();
                if to_part.is_empty() || !is_valid_participant_id(to_part) {
                    continue;
                }
                let to = to_part.to_string();
                let lbl = after[colon_idx + 1..].trim().to_string();
                let label = if lbl.is_empty() { None } else { Some(lbl) };
                (to, label)
            } else {
                let to_part = after.trim();
                if to_part.is_empty() || !is_valid_participant_id(to_part) {
                    continue;
                }
                (to_part.to_string(), None)
            };

            return Some(Ok((from, to, label, line_style, arrow)));
        }
    }

    None
}

/// Check if `s` is a valid PlantUML participant identifier (bare, unquoted).
/// Accepts alphanumeric, underscore, and Unicode letters (for Japanese names etc.).
fn is_valid_participant_id(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    // Allow alphanumeric, underscore, hyphen, and any non-ASCII Unicode letters.
    s.chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use kozue_ir::{ArrowType, LineStyle, SequenceItem};

    fn parse_ok(src: &str) -> SequenceDiagram {
        match parse(src).expect("should parse without errors") {
            Diagram::Sequence(s) => s,
            _ => panic!("expected Sequence diagram"),
        }
    }

    fn parse_err(src: &str) -> Vec<Diagnostic> {
        parse(src).expect_err("should return errors")
    }

    // -----------------------------------------------------------------------
    // Basic parsing
    // -----------------------------------------------------------------------

    #[test]
    fn basic_two_participant_sequence() {
        let src = "@startuml\nparticipant Alice\nparticipant Bob\nAlice -> Bob : hello\n@enduml\n";
        let s = parse_ok(src);
        assert_eq!(s.participants.len(), 2);
        assert!(s.participants.contains_key("Alice"));
        assert!(s.participants.contains_key("Bob"));
        assert_eq!(s.items.len(), 1);
        let SequenceItem::Message(ref m) = s.items[0] else {
            panic!()
        };
        assert_eq!(m.from, "Alice");
        assert_eq!(m.to, "Bob");
        assert_eq!(m.label.as_deref(), Some("hello"));
        assert_eq!(m.line, LineStyle::Solid);
        assert_eq!(m.arrow, ArrowType::Triangle);
    }

    #[test]
    fn as_alias_sets_id_and_label() {
        let src = "@startuml\nparticipant Alice as A\nparticipant Bob as B\nA -> B : hi\n@enduml\n";
        let s = parse_ok(src);
        assert_eq!(s.participants.len(), 2);
        // id is the alias
        assert!(s.participants.contains_key("A"));
        assert_eq!(s.participants["A"].label, "Alice");
        assert!(s.participants.contains_key("B"));
        assert_eq!(s.participants["B"].label, "Bob");
    }

    #[test]
    fn quoted_display_name_with_alias() {
        let src =
            "@startuml\nparticipant \"Web Browser\" as WB\nparticipant Server\nWB -> Server : GET /\n@enduml\n";
        let s = parse_ok(src);
        assert!(s.participants.contains_key("WB"));
        assert_eq!(s.participants["WB"].label, "Web Browser");
        assert!(s.participants.contains_key("Server"));
    }

    #[test]
    fn auto_declare_participants_on_first_use() {
        // No explicit participant declarations — auto-declared via message.
        let src = "@startuml\nA -> B : test\n@enduml\n";
        let s = parse_ok(src);
        assert_eq!(s.participants.len(), 2);
        assert!(s.participants.contains_key("A"));
        assert!(s.participants.contains_key("B"));
        assert_eq!(s.participants["A"].label, "A");
        assert_eq!(s.participants["B"].label, "B");
    }

    #[test]
    fn self_message() {
        let src = "@startuml\nparticipant A\nA -> A : think\n@enduml\n";
        let s = parse_ok(src);
        assert_eq!(s.items.len(), 1);
        let SequenceItem::Message(ref m) = s.items[0] else {
            panic!()
        };
        assert_eq!(m.from, "A");
        assert_eq!(m.to, "A");
        assert_eq!(m.label.as_deref(), Some("think"));
    }

    #[test]
    fn solid_arrow_maps_to_solid_triangle() {
        let src = "@startuml\nA -> B : msg\n@enduml\n";
        let s = parse_ok(src);
        let SequenceItem::Message(ref m) = s.items[0] else {
            panic!()
        };
        assert_eq!(m.line, LineStyle::Solid);
        assert_eq!(m.arrow, ArrowType::Triangle);
    }

    #[test]
    fn dashed_arrow_maps_to_dashed_triangle() {
        let src = "@startuml\nA --> B : msg\n@enduml\n";
        let s = parse_ok(src);
        let SequenceItem::Message(ref m) = s.items[0] else {
            panic!()
        };
        assert_eq!(m.line, LineStyle::Dashed);
        assert_eq!(m.arrow, ArrowType::Triangle);
    }

    #[test]
    fn thin_solid_arrow_maps_to_solid_triangle() {
        // ->> maps to Solid + Triangle (thin arrowhead not rendered)
        let src = "@startuml\nA ->> B : msg\n@enduml\n";
        let s = parse_ok(src);
        let SequenceItem::Message(ref m) = s.items[0] else {
            panic!()
        };
        assert_eq!(m.line, LineStyle::Solid);
        assert_eq!(m.arrow, ArrowType::Triangle);
    }

    #[test]
    fn thin_dashed_arrow_maps_to_dashed_triangle() {
        // -->> maps to Dashed + Triangle
        let src = "@startuml\nA -->> B : msg\n@enduml\n";
        let s = parse_ok(src);
        let SequenceItem::Message(ref m) = s.items[0] else {
            panic!()
        };
        assert_eq!(m.line, LineStyle::Dashed);
        assert_eq!(m.arrow, ArrowType::Triangle);
    }

    #[test]
    fn actor_keyword_works_like_participant() {
        let src = "@startuml\nactor User\nparticipant System\nUser -> System : login\n@enduml\n";
        let s = parse_ok(src);
        assert!(s.participants.contains_key("User"));
        assert_eq!(s.participants["User"].label, "User");
    }

    #[test]
    fn actor_with_alias() {
        let src = "@startuml\nactor \"End User\" as U\nU -> System : click\n@enduml\n";
        let s = parse_ok(src);
        assert!(s.participants.contains_key("U"));
        assert_eq!(s.participants["U"].label, "End User");
    }

    #[test]
    fn icon_variant_keywords_map_to_participant() {
        let src = "@startuml\nboundary FE\ncontrol BE\nentity DB\nFE -> BE : req\nBE -> DB : query\n@enduml\n";
        let s = parse_ok(src);
        assert!(s.participants.contains_key("FE"));
        assert!(s.participants.contains_key("BE"));
        assert!(s.participants.contains_key("DB"));
    }

    #[test]
    fn message_without_label() {
        let src = "@startuml\nA -> B\n@enduml\n";
        let s = parse_ok(src);
        let SequenceItem::Message(ref m) = s.items[0] else {
            panic!()
        };
        assert_eq!(m.label, None);
    }

    // -----------------------------------------------------------------------
    // Error cases
    // -----------------------------------------------------------------------

    #[test]
    fn missing_startuml_is_error() {
        let src = "participant A\nA -> B : hi\n@enduml\n";
        let errs = parse_err(src);
        assert!(
            errs.iter().any(|e| e.message.contains("@startuml")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn missing_enduml_is_error() {
        let src = "@startuml\nA -> B : hi\n";
        let errs = parse_err(src);
        assert!(
            errs.iter().any(|e| e.message.contains("@enduml")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn startmindmap_is_unsupported() {
        let src = "@startmindmap\n* root\n@endmindmap\n";
        let errs = parse_err(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unsupported") && e.message.contains("mindmap")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn startgantt_is_unsupported() {
        let src = "@startgantt\nProject starts 2024-01-01\n@endgantt\n";
        let errs = parse_err(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unsupported") && e.message.contains("gantt")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn startjson_is_unsupported() {
        let src = "@startjson\n{}\n@endjson\n";
        let errs = parse_err(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unsupported") || e.message.contains("json")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn preprocessor_include_is_unsupported() {
        let src = "@startuml\n!include other.puml\nA -> B : hi\n@enduml\n";
        let errs = parse_err(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unsupported") && e.message.contains("preprocessor")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn preprocessor_define_is_unsupported() {
        let src = "@startuml\n!define FOO bar\nA -> B : FOO\n@enduml\n";
        let errs = parse_err(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unsupported") && e.message.contains("preprocessor")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn note_is_unsupported() {
        let src = "@startuml\nparticipant A\nnote over A: some text\nA -> A : ok\n@enduml\n";
        let errs = parse_err(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unsupported")
                    && e.message.to_lowercase().contains("note")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn alt_is_unsupported() {
        let src = "@startuml\nA -> B : req\nalt success\n  B --> A : ok\nend\n@enduml\n";
        let errs = parse_err(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unsupported") && e.message.contains("alt")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn loop_is_unsupported() {
        let src = "@startuml\nloop every second\n  A -> A : tick\nend\n@enduml\n";
        let errs = parse_err(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unsupported") && e.message.contains("loop")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn end_silently_skipped() {
        // `end` alone on a line should be silently skipped (not cause an error).
        // Body: participant A, then an implicit alt-end that got orphaned.
        let src = "@startuml\nparticipant A\nend\nA -> A : ok\n@enduml\n";
        // Should parse successfully (no error for bare `end`).
        let s = parse_ok(src);
        assert_eq!(s.items.len(), 1);
    }

    #[test]
    fn line_comment_is_stripped() {
        // `'` comment should be stripped; the message should parse.
        let src = "@startuml\n' this is a comment\nparticipant A\nparticipant B\nA -> B : msg ' inline comment\n@enduml\n";
        let s = parse_ok(src);
        assert_eq!(s.items.len(), 1);
        let SequenceItem::Message(ref m) = s.items[0] else {
            panic!()
        };
        // Label should contain "msg" (trailing space before comment is preserved, comment stripped).
        let label = m.label.as_deref().unwrap_or("");
        assert!(
            label.contains("msg"),
            "label should contain 'msg', got: {label:?}"
        );
        assert!(
            !label.contains("inline comment"),
            "label must not contain comment text"
        );
    }

    #[test]
    fn block_comment_is_stripped() {
        let src = "@startuml\n/' this is a block comment '/\nA -> B : hello\n@enduml\n";
        let s = parse_ok(src);
        assert_eq!(s.items.len(), 1);
    }

    #[test]
    fn multiline_block_comment_is_stripped() {
        let src =
            "@startuml\n/'\nline 1 of comment\nline 2 of comment\n'/\nA -> B : hello\n@enduml\n";
        let s = parse_ok(src);
        assert_eq!(s.items.len(), 1);
    }

    #[test]
    fn three_participants_with_solid_and_dashed() {
        let src = "@startuml\nparticipant Alice as A\nparticipant Bob as B\nparticipant Carol as C\nA -> B : request\nB --> A : response\nA -> C : notify\n@enduml\n";
        let s = parse_ok(src);
        assert_eq!(s.participants.len(), 3);
        assert_eq!(s.items.len(), 3);
        let SequenceItem::Message(ref m0) = s.items[0] else {
            panic!()
        };
        assert_eq!(m0.line, LineStyle::Solid);
        let SequenceItem::Message(ref m1) = s.items[1] else {
            panic!()
        };
        assert_eq!(m1.line, LineStyle::Dashed);
    }

    #[test]
    fn par_is_unsupported_not_confused_with_participant() {
        // `par` should be unsupported but `participant` should still work.
        let src = "@startuml\nparticipant Alice\npar\nAlice -> Alice : ok\nend\n@enduml\n";
        let errs = parse_err(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unsupported") && e.message.contains("par")),
            "`par` should be unsupported, got: {errs:?}"
        );
    }

    #[test]
    fn startuml_with_name_is_accepted() {
        // @startuml SomeName — name is accepted and ignored.
        let src = "@startuml MyDiagram\nA -> B : hi\n@enduml\n";
        let s = parse_ok(src);
        assert_eq!(s.items.len(), 1);
    }

    #[test]
    fn japanese_labels_in_messages() {
        let src = "@startuml\nparticipant クライアント as C\nparticipant サーバ as S\nC -> S : ログイン\nS --> C : 成功\n@enduml\n";
        let s = parse_ok(src);
        assert_eq!(s.participants.len(), 2);
        assert_eq!(s.participants["C"].label, "クライアント");
        assert_eq!(s.items.len(), 2);
        let SequenceItem::Message(ref m0) = s.items[0] else {
            panic!()
        };
        assert_eq!(m0.label.as_deref(), Some("ログイン"));
    }

    // -----------------------------------------------------------------------
    // Regression: comment masking must not silently drop input (BLOCKER #1)
    // -----------------------------------------------------------------------

    #[test]
    fn block_open_inside_line_comment_does_not_swallow_input() {
        // A `/'` sitting inside a `'` line comment must NOT open a block comment
        // and eat the following real message.
        let src = "@startuml\n' TODO: convert to /' block\nSECRET -> LEAK : must not vanish\n' done '/\nA -> B : visible\n@enduml\n";
        let s = parse_ok(src);
        // Both real messages must survive.
        assert_eq!(s.items.len(), 2, "no message may be silently dropped");
        let froms: Vec<&str> = s
            .items
            .iter()
            .filter_map(|it| match it {
                SequenceItem::Message(m) => Some(m.from.as_str()),
                _ => None,
            })
            .collect();
        assert!(froms.contains(&"SECRET"), "got: {froms:?}");
        assert!(froms.contains(&"A"), "got: {froms:?}");
    }

    #[test]
    fn block_open_inside_quoted_name_does_not_swallow_input() {
        // `/'` inside a quoted display name is literal, not a block opener.
        let src = "@startuml\nparticipant \"a /' b\" as X\nX -> Y : survives\n@enduml\n";
        let s = parse_ok(src);
        assert_eq!(s.participants["X"].label, "a /' b");
        assert_eq!(s.items.len(), 1, "message after quoted /' must survive");
    }

    // -----------------------------------------------------------------------
    // Regression: diagnostic spans stay aligned after a block comment (BLOCKER #2)
    // -----------------------------------------------------------------------

    #[test]
    fn unterminated_block_comment_is_error() {
        // A `/'` that is never closed masks to EOF; this must be diagnosed, not
        // silently swallowed (including the swallowed message and @enduml).
        let src = "@startuml\nA -> B : hi\n/' oops never closed\nC -> D : lost\n@enduml\n";
        let errs = parse_err(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unterminated block comment")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn error_span_is_correct_after_block_comment() {
        let src = "@startuml\n/' a block comment here '/\nnote over A : unsupported\n@enduml\n";
        let errs = parse_err(src);
        let note_err = errs
            .iter()
            .find(|e| e.message.contains("note"))
            .expect("expected a note error");
        // The span must underline the actual `note` occurrence in the ORIGINAL source.
        assert_eq!(
            &src[note_err.span.clone()],
            "note over A : unsupported",
            "span must map to the note line, not shift due to the block comment"
        );
    }

    // -----------------------------------------------------------------------
    // Regression: malformed participant decls error instead of misparsing
    // -----------------------------------------------------------------------

    #[test]
    fn dangling_as_is_error_not_phantom_participant() {
        let src = "@startuml\nparticipant Foo as\nFoo -> Bar : hi\n@enduml\n";
        let errs = parse_err(src);
        assert!(
            errs.iter().any(|e| e.message.contains("unrecognised")),
            "trailing `as` must be an error, got: {errs:?}"
        );
    }

    #[test]
    fn duplicate_participant_id_is_error() {
        let src = "@startuml\nparticipant Alice as X\nparticipant Bob as X\nX -> X : hi\n@enduml\n";
        let errs = parse_err(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("duplicate participant")),
            "duplicate id must be an error, got: {errs:?}"
        );
    }

    #[test]
    fn multiple_errors_are_all_collected() {
        // Both note and alt should produce errors — all collected before returning.
        let src = "@startuml\nnote over A: text\nalt success\nA -> B : hi\nend\n@enduml\n";
        let errs = parse_err(src);
        assert!(errs.len() >= 2, "expected multiple errors, got: {errs:?}");
    }
}
