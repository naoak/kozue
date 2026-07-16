//! Native kozue DSL parser for `class <name> { ... }` diagrams.
//!
//! This is a standalone, byte-offset-based parser (not chumsky): once the
//! header keyword selects `class`, the grammar is different enough from
//! graph/sequence/state (nested member blocks, UML relation operators) that
//! sharing the chumsky statement grammar would be more confusing than
//! helpful. Diagnostics use exact byte spans into the original source.
//!
//! ```text
//! class orders {
//!   class Order {
//!     +id: Int
//!     +total: Money
//!     +submit(): void
//!   }
//!   interface Payable { +pay(): void }
//!
//!   Customer "1" o-- "*" Order : "places"
//!   Dog --|> Animal
//! }
//! ```

use kozue_ir::{ClassDiagram, ClassNode, ClassRelation, Diagram, Direction, EndMarker, LineStyle};

use crate::CompileError;

/// DSL class-relation connectors (PlantUML-style): `(token, from_marker, to_marker, dashed)`.
/// Both spelling directions of each relation kind are accepted, with the marker
/// placed on the end the token points at (e.g. `A <|-- B` puts the hollow
/// triangle at A's end, `A --|> B` at B's end).
const CLASS_CONNECTORS: &[(&str, EndMarker, EndMarker, bool)] = &[
    // Generalization / realization (hollow triangle).
    ("<|--", EndMarker::HollowTriangle, EndMarker::None, false),
    ("--|>", EndMarker::None, EndMarker::HollowTriangle, false),
    ("<|..", EndMarker::HollowTriangle, EndMarker::None, true),
    ("..|>", EndMarker::None, EndMarker::HollowTriangle, true),
    // Composition (filled diamond).
    ("*--", EndMarker::FilledDiamond, EndMarker::None, false),
    ("--*", EndMarker::None, EndMarker::FilledDiamond, false),
    // Aggregation (hollow diamond).
    ("o--", EndMarker::HollowDiamond, EndMarker::None, false),
    ("--o", EndMarker::None, EndMarker::HollowDiamond, false),
    // Association / dependency (open arrow).
    ("-->", EndMarker::None, EndMarker::OpenArrow, false),
    ("<--", EndMarker::OpenArrow, EndMarker::None, false),
    ("..>", EndMarker::None, EndMarker::OpenArrow, true),
    ("<..", EndMarker::OpenArrow, EndMarker::None, true),
    // Plain association (no markers).
    ("--", EndMarker::None, EndMarker::None, false),
    ("..", EndMarker::None, EndMarker::None, true),
];

fn err(msg: impl Into<String>, span: std::ops::Range<usize>) -> CompileError {
    CompileError {
        message: msg.into(),
        span,
        secondary: None,
    }
}

/// Parse a `class <name> { ... }` diagram from `src`. The caller has already
/// confirmed the header keyword is `class`.
///
/// Internally this scanner works in byte offsets (needed for correct UTF-8
/// slicing); errors are converted to character offsets before being returned,
/// matching the convention the rest of `kozue-dsl` (chumsky) uses.
pub fn parse(src: &str) -> Result<Diagram, Vec<CompileError>> {
    parse_bytes(src).map_err(|errs| to_char_spans(src, errs))
}

fn parse_bytes(src: &str) -> Result<Diagram, Vec<CompileError>> {
    let mut idx;
    let Some((kw, kw_span)) = crate::peek_header_keyword(src) else {
        return Err(vec![err("expected `class` keyword", 0..src.len().max(1))]);
    };
    if kw != "class" {
        return Err(vec![err(
            format!("expected `class` keyword, got `{kw}`"),
            kw_span,
        )]);
    }
    idx = kw_span.end;

    let Some((_name, name_span)) = peek_ident(src, idx) else {
        return Err(vec![err(
            "expected a diagram name after `class`",
            idx..(idx + 1).min(src.len().max(idx + 1)),
        )]);
    };
    idx = name_span.end;
    idx = skip_ws_comments(src, idx);
    if !src[idx..].starts_with('{') {
        return Err(vec![err(
            "expected `{` after the diagram name",
            idx..(idx + 1).min(src.len()),
        )]);
    }
    let open_brace = idx;
    let Some(close_brace) = find_matching_brace(src, open_brace) else {
        return Err(vec![err(
            "unterminated diagram block (missing `}`)",
            open_brace..open_brace + 1,
        )]);
    };

    let mut errors: Vec<CompileError> = Vec::new();
    let after = skip_ws_comments(src, close_brace + 1);
    if after != src.len() {
        errors.push(err(
            format!(
                "unexpected trailing tokens after closing `}}`: `{}`",
                src[after..].chars().take(40).collect::<String>()
            ),
            after..src.len(),
        ));
    }

    let body_offset = open_brace + 1;
    let body = &src[body_offset..close_brace];
    let lines = logical_lines(body, body_offset);

    let mut diagram = ClassDiagram::new(Direction::Down);
    let ensure_class = |diagram: &mut ClassDiagram, id: &str| {
        if !diagram.classes.contains_key(id) {
            diagram.classes.insert(
                id.to_string().into(),
                ClassNode::new(id.to_string(), id.to_string()),
            );
        }
    };

    let mut relations: Vec<(ParsedRelation, std::ops::Range<usize>)> = Vec::new();

    let mut i = 0usize;
    while i < lines.len() {
        let (offset, line) = lines[i];
        let span = offset..offset + line.len();

        if let Some((stereotype, rest)) = class_decl_keyword(line) {
            // Three shapes: bare `class Foo`, inline `class Foo { a; b }`
            // (single line, members `;`-separated), or a multi-line block
            // `class Foo {` ... `}`.
            let block_kind = if let Some(open_idx) = rest.find('{') {
                if let Some(inner) = rest[open_idx + 1..].strip_suffix('}') {
                    BlockKind::Inline(rest[..open_idx].trim(), inner.trim())
                } else if rest.trim_end().ends_with('{') {
                    BlockKind::Multiline(rest[..open_idx].trim())
                } else {
                    BlockKind::Malformed
                }
            } else {
                BlockKind::None(rest)
            };

            let name = match block_kind {
                BlockKind::Inline(n, _) | BlockKind::Multiline(n) | BlockKind::None(n) => n,
                BlockKind::Malformed => {
                    errors.push(err(
                        format!(
                            "syntax error: malformed class body `{}`",
                            line.chars().take(40).collect::<String>()
                        ),
                        span,
                    ));
                    i += 1;
                    continue;
                }
            };
            if !is_ident(name) {
                errors.push(err(
                    format!(
                        "expected a class identifier, got `{}`",
                        name.chars().take(40).collect::<String>()
                    ),
                    span,
                ));
                i += 1;
                continue;
            }
            ensure_class(&mut diagram, name);
            if let Some(st) = stereotype {
                diagram.classes[name].stereotype = Some(st.to_string());
            }

            match block_kind {
                BlockKind::None(_) => {
                    i += 1;
                }
                BlockKind::Inline(_, inner) => {
                    let node = &mut diagram.classes[name];
                    let mut attrs = std::mem::take(&mut node.attributes);
                    let mut methods = std::mem::take(&mut node.methods);
                    for member in inner.split(';') {
                        let member = member.trim();
                        if member.is_empty() {
                            continue;
                        }
                        if let Err(msg) = parse_class_member(member, &mut attrs, &mut methods) {
                            errors.push(err(msg, span.clone()));
                        }
                    }
                    let node = &mut diagram.classes[name];
                    node.attributes = attrs;
                    node.methods = methods;
                    i += 1;
                }
                BlockKind::Multiline(_) => {
                    i += 1;
                    loop {
                        if i >= lines.len() {
                            errors.push(err(
                                format!("unterminated `{name} {{ ... }}` block (missing `}}`)"),
                                span.clone(),
                            ));
                            break;
                        }
                        let (moff, mline) = lines[i];
                        if mline.trim() == "}" {
                            i += 1;
                            break;
                        }
                        let node = &mut diagram.classes[name];
                        let mut attrs = std::mem::take(&mut node.attributes);
                        let mut methods = std::mem::take(&mut node.methods);
                        if let Err(msg) = parse_class_member(mline, &mut attrs, &mut methods) {
                            errors.push(err(msg, moff..moff + mline.len()));
                        }
                        let node = &mut diagram.classes[name];
                        node.attributes = attrs;
                        node.methods = methods;
                        i += 1;
                    }
                }
                BlockKind::Malformed => unreachable!(),
            }
            continue;
        }

        if let Some(result) = try_parse_relation(line) {
            match result {
                Ok(rel) => relations.push((rel, span)),
                Err(msg) => errors.push(err(msg, span)),
            }
            i += 1;
            continue;
        }

        errors.push(err(
            format!(
                "syntax error: unrecognised statement `{}`",
                line.chars().take(40).collect::<String>()
            ),
            span,
        ));
        i += 1;
    }

    for (rel, span) in relations {
        if rel.from == rel.to {
            errors.push(err(
                format!(
                    "self relations are not supported in class diagrams (`{}` -> `{}`)",
                    rel.from, rel.to
                ),
                span,
            ));
            continue;
        }
        ensure_class(&mut diagram, &rel.from);
        ensure_class(&mut diagram, &rel.to);
        diagram.relations.push(ClassRelation::new(
            rel.from,
            rel.to,
            rel.from_marker,
            rel.to_marker,
            if rel.dashed {
                LineStyle::Dashed
            } else {
                LineStyle::Solid
            },
            rel.label,
            rel.from_mult,
            rel.to_mult,
        ));
    }

    if errors.is_empty() {
        Ok(Diagram::Class(diagram))
    } else {
        Err(errors)
    }
}

/// Convert every error span from byte offsets (used internally by this
/// scanner) to **character** offsets, matching the convention the rest of
/// `kozue-dsl` (chumsky) uses for [`CompileError::span`] — callers such as
/// `kozue-wasm`/`kozue-lsp` treat all `kozue_dsl::parse` spans uniformly as
/// character indices.
pub(crate) fn to_char_spans(src: &str, errors: Vec<CompileError>) -> Vec<CompileError> {
    errors
        .into_iter()
        .map(|e| CompileError {
            message: e.message,
            span: byte_to_char(src, e.span.start)..byte_to_char(src, e.span.end),
            secondary: e
                .secondary
                .map(|(s, m)| (byte_to_char(src, s.start)..byte_to_char(src, s.end), m)),
        })
        .collect()
}

fn byte_to_char(src: &str, byte_idx: usize) -> usize {
    src[..byte_idx.min(src.len())].chars().count()
}

// ---------------------------------------------------------------------------
// Small shared scanning helpers (byte-offset based; no chumsky).
// ---------------------------------------------------------------------------

fn skip_ws_comments(src: &str, mut idx: usize) -> usize {
    loop {
        let rest = &src[idx..];
        let ws_len = rest.len() - rest.trim_start().len();
        idx += ws_len;
        if src[idx..].starts_with("//") {
            let nl = src[idx..].find('\n').map(|o| idx + o).unwrap_or(src.len());
            idx = nl;
            continue;
        }
        break;
    }
    idx
}

fn peek_ident(src: &str, idx: usize) -> Option<(&str, std::ops::Range<usize>)> {
    let idx = skip_ws_comments(src, idx);
    let start = idx;
    let end = src[idx..]
        .char_indices()
        .find(|&(_, c)| !(c.is_alphanumeric() || c == '_'))
        .map(|(o, _)| idx + o)
        .unwrap_or(src.len());
    if end == start {
        return None;
    }
    Some((&src[start..end], start..end))
}

/// Find the byte offset of the `}` matching the `{` at `open_idx`.
///
/// This tracks nesting **line by line**, not by counting every raw `{`/`}`
/// character: ER crow's-foot relation tokens (e.g. `o{`, `}o`) legitimately
/// contain literal `{`/`}` glyphs that are not block delimiters. By
/// convention every real block-opener line ends with a bare `{` and every
/// block-closer is a line whose only content is `}` (after stripping a
/// trailing `//` comment) — the same convention `class`/`entity` member
/// blocks use throughout this parser — so scanning for that shape is both
/// simpler and correct where raw brace-counting is not.
pub(crate) fn find_matching_brace(src: &str, open_idx: usize) -> Option<usize> {
    let mut offset = 0usize;
    let mut depth = 0i32;
    for raw in src.split_inclusive('\n') {
        let line_start = offset;
        let line_end = offset + raw.len();
        offset = line_end;
        if depth == 0 {
            if line_start <= open_idx && open_idx < line_end {
                depth = 1;
            }
            continue;
        }
        let stripped = strip_line_comment(raw.trim_end_matches('\n'));
        let trimmed = stripped.trim();
        if trimmed == "}" {
            depth -= 1;
            if depth == 0 {
                let rel = raw.find('}')?;
                return Some(line_start + rel);
            }
        } else if trimmed.ends_with('{') {
            depth += 1;
        }
    }
    None
}

/// Strip a trailing `// ...` comment (outside of quoted strings) from a line.
pub(crate) fn strip_line_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut in_quotes = false;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                in_quotes = !in_quotes;
                i += 1;
            }
            b'/' if !in_quotes && i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                return line[..i].trim_end();
            }
            _ => i += 1,
        }
    }
    line
}

/// Split `body` into non-empty, comment-stripped logical lines with their
/// absolute byte offsets into the original source.
pub(crate) fn logical_lines(body: &str, body_offset: usize) -> Vec<(usize, &str)> {
    let mut result = Vec::new();
    let mut offset = body_offset;
    for raw in body.split('\n') {
        let stripped = strip_line_comment(raw);
        let trimmed = stripped.trim();
        if !trimmed.is_empty() {
            // `trimmed` may start later than `stripped`/`raw` (leading
            // whitespace) — the span must point at the trimmed content, not
            // the raw line, so it lands exactly on the reported text.
            let leading_ws = stripped.len() - stripped.trim_start().len();
            result.push((offset + leading_ws, trimmed));
        }
        offset += raw.len() + 1;
    }
    result
}

pub(crate) fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_alphanumeric() || c == '_') && !s.is_empty()
}

/// Split a string into whitespace-separated tokens, treating a `"..."` run
/// as a single token.
pub(crate) fn tokenize_ws_quoted(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for c in s.chars() {
        if c == '"' {
            in_quotes = !in_quotes;
            cur.push(c);
        } else if c.is_whitespace() && !in_quotes {
            if !cur.is_empty() {
                tokens.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
}

pub(crate) fn find_unquoted_colon(s: &str) -> Option<usize> {
    let mut in_quotes = false;
    for (i, c) in s.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            ':' if !in_quotes => return Some(i),
            _ => {}
        }
    }
    None
}

/// Parse a `"..."` string literal that consumes the entire (trimmed) input,
/// with `\"`/`\\` escapes. Returns `None` if malformed.
pub(crate) fn parse_quoted_string(s: &str) -> Option<String> {
    let s = s.trim();
    if !s.starts_with('"') {
        return None;
    }
    let mut chars = s.char_indices().skip(1);
    let mut out = String::new();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => {
                if s[i + 1..].trim().is_empty() {
                    return Some(out);
                }
                return None;
            }
            '\\' => match chars.next() {
                Some((_, next)) if next == '"' || next == '\\' => out.push(next),
                _ => return None,
            },
            other => out.push(other),
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Class declaration / member parsing
// ---------------------------------------------------------------------------

fn strip_kw<'a>(s: &'a str, kw: &str) -> Option<&'a str> {
    let rest = s.strip_prefix(kw)?;
    if rest.is_empty() || rest.starts_with(|c: char| c.is_whitespace()) {
        Some(rest.trim_start())
    } else {
        None
    }
}

/// Decode the leading declaration keyword: `class` / `interface` /
/// `abstract class` / `abstract` / `enum`. Returns `(stereotype, rest)`.
fn class_decl_keyword(trimmed: &str) -> Option<(Option<&'static str>, &str)> {
    if let Some(rest) = strip_kw(trimmed, "abstract") {
        if let Some(rest2) = strip_kw(rest, "class") {
            return Some((Some("abstract"), rest2));
        }
        return Some((Some("abstract"), rest));
    }
    if let Some(rest) = strip_kw(trimmed, "interface") {
        return Some((Some("interface"), rest));
    }
    if let Some(rest) = strip_kw(trimmed, "enum") {
        return Some((Some("enumeration"), rest));
    }
    if let Some(rest) = strip_kw(trimmed, "class") {
        return Some((None, rest));
    }
    None
}

/// Parse a class member line: `[+-#~]? name: Type` (attribute) or
/// `[+-#~]? name(args): Ret` (method — presence of `(` decides).
fn parse_class_member(
    trimmed: &str,
    attributes: &mut Vec<String>,
    methods: &mut Vec<String>,
) -> Result<(), String> {
    let vis = trimmed.chars().next().filter(|c| "+-#~".contains(*c));
    let rest = if let Some(v) = vis {
        trimmed[v.len_utf8()..].trim_start()
    } else {
        trimmed
    };
    let vis_str = vis.map(|c| c.to_string()).unwrap_or_default();

    if rest.is_empty() {
        return Err(format!(
            "syntax error: unrecognised class member `{}`",
            trimmed.chars().take(40).collect::<String>()
        ));
    }

    if let Some(paren_idx) = rest.find('(') {
        let name = rest[..paren_idx].trim();
        let Some(close_rel) = rest[paren_idx..].find(')') else {
            return Err("unterminated `(` in method member (missing `)`)".to_string());
        };
        let close = paren_idx + close_rel;
        let args = &rest[paren_idx + 1..close];
        let after = rest[close + 1..].trim();
        let ret = after.trim_start_matches(':').trim();
        let formatted = if ret.is_empty() {
            format!("{vis_str}{name}({args})")
        } else {
            format!("{vis_str}{name}({args}): {ret}")
        };
        methods.push(formatted);
    } else if let Some(ci) = rest.find(':') {
        let name = rest[..ci].trim();
        let ty = rest[ci + 1..].trim();
        attributes.push(format!("{vis_str}{name}: {ty}"));
    } else {
        attributes.push(format!("{vis_str}{rest}"));
    }
    Ok(())
}

/// The shape of a `class`/`interface`/`abstract [class]`/`enum` declaration
/// line's body, determined purely from the text after the keyword.
enum BlockKind<'a> {
    /// `class Foo` — no body at all.
    None(&'a str),
    /// `class Foo { a; b }` — single line, `;`-separated members.
    Inline(&'a str, &'a str),
    /// `class Foo {` — a multi-line block, closed by a standalone `}` line.
    Multiline(&'a str),
    /// Has a `{` but no well-formed close on the same line and the line
    /// doesn't end with a bare `{` either (e.g. stray trailing tokens).
    Malformed,
}

struct ParsedRelation {
    from: String,
    to: String,
    from_marker: EndMarker,
    to_marker: EndMarker,
    dashed: bool,
    label: Option<String>,
    from_mult: Option<String>,
    to_mult: Option<String>,
}

fn split_id_and_mult(tokens: &[String]) -> Result<(String, Option<String>), String> {
    let mut id: Option<String> = None;
    let mut mult: Option<String> = None;
    for t in tokens {
        if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
            if mult.is_some() {
                return Err("multiple multiplicities on one side of a relation".to_string());
            }
            mult = Some(t[1..t.len() - 1].to_string());
        } else {
            if id.is_some() {
                return Err("multiple identifiers on one side of a relation".to_string());
            }
            id = Some(t.clone());
        }
    }
    let id = id.ok_or_else(|| "missing identifier in relation".to_string())?;
    if !is_ident(&id) {
        return Err(format!("invalid identifier `{id}` in relation"));
    }
    Ok((id, mult))
}

fn try_parse_relation(trimmed: &str) -> Option<Result<ParsedRelation, String>> {
    let (rel_part, label) = match find_unquoted_colon(trimmed) {
        Some(ci) => {
            let after = trimmed[ci + 1..].trim();
            match parse_quoted_string(after) {
                Some(s) => (trimmed[..ci].trim(), Some(s)),
                None => {
                    return Some(Err(format!(
                        "expected a quoted string label after `:`, got `{}`",
                        after.chars().take(40).collect::<String>()
                    )));
                }
            }
        }
        None => (trimmed, None),
    };

    let tokens = tokenize_ws_quoted(rel_part);
    let conn_idx = tokens
        .iter()
        .position(|t| CLASS_CONNECTORS.iter().any(|(tok, ..)| tok == t))?;
    let &(_, from_marker, to_marker, dashed) = CLASS_CONNECTORS
        .iter()
        .find(|(tok, ..)| *tok == tokens[conn_idx])
        .unwrap();

    let from_tokens = &tokens[..conn_idx];
    let to_tokens = &tokens[conn_idx + 1..];
    let (from, from_mult) = match split_id_and_mult(from_tokens) {
        Ok(v) => v,
        Err(e) => return Some(Err(e)),
    };
    let (to, to_mult) = match split_id_and_mult(to_tokens) {
        Ok(v) => v,
        Err(e) => return Some(Err(e)),
    };
    Some(Ok(ParsedRelation {
        from,
        to,
        from_marker,
        to_marker,
        dashed,
        label,
        from_mult,
        to_mult,
    }))
}
