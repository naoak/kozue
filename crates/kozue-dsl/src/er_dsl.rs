//! Native kozue DSL parser for `er <name> { ... }` diagrams.
//!
//! Standalone, byte-offset-based parser (mirrors `class_dsl`). Grammar:
//!
//! ```text
//! er shop {
//!   entity Customer {
//!     id: Int PK
//!     name: String
//!     email: String UK
//!   }
//!   entity Order { id: Int PK }
//!
//!   Customer ||--o{ Order : "places"
//! }
//! ```

use kozue_ir::{Diagram, EndMarker, ErAttribute, ErDiagram, ErEntity, ErRelation, LineStyle};

use crate::class_dsl::{
    find_matching_brace, find_unquoted_colon, is_ident, logical_lines, parse_quoted_string,
    tokenize_ws_quoted,
};
use crate::CompileError;

fn err(msg: impl Into<String>, span: std::ops::Range<usize>) -> CompileError {
    CompileError {
        message: msg.into(),
        span,
        secondary: None,
    }
}

/// Decode an ER crow's-foot connector token, e.g. `||--o{`.
fn parse_crowfoot_token(tok: &str) -> Option<(EndMarker, EndMarker, LineStyle)> {
    let (mid, dashed) = if let Some(idx) = tok.find("--") {
        (idx, false)
    } else if let Some(idx) = tok.find("..") {
        (idx, true)
    } else {
        return None;
    };
    if mid != 2 || tok.len() != mid + 4 {
        return None;
    }
    let left = &tok[..mid];
    let right = &tok[mid + 2..];
    let from_marker = match left {
        "||" => EndMarker::ErOne,
        "o|" => EndMarker::ErZeroOrOne,
        "}o" => EndMarker::ErZeroOrMany,
        "}|" => EndMarker::ErOneOrMany,
        _ => return None,
    };
    let to_marker = match right {
        "||" => EndMarker::ErOne,
        "|o" => EndMarker::ErZeroOrOne,
        "o{" => EndMarker::ErZeroOrMany,
        "|{" => EndMarker::ErOneOrMany,
        _ => return None,
    };
    Some((
        from_marker,
        to_marker,
        if dashed {
            LineStyle::Dashed
        } else {
            LineStyle::Solid
        },
    ))
}

struct ParsedRelation {
    from: String,
    to: String,
    from_marker: EndMarker,
    to_marker: EndMarker,
    line: LineStyle,
    label: Option<String>,
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
    let tokens: Vec<&str> = rel_part.split_whitespace().collect();
    if tokens.len() != 3 {
        return None;
    }
    let (from_marker, to_marker, line) = parse_crowfoot_token(tokens[1])?;
    if !is_ident(tokens[0]) || !is_ident(tokens[2]) {
        return None;
    }
    Some(Ok(ParsedRelation {
        from: tokens[0].to_string(),
        to: tokens[2].to_string(),
        from_marker,
        to_marker,
        line,
        label,
    }))
}

/// Parse one entity attribute line: `name: Type [PK|FK|UK]... ["comment"]`.
fn parse_attr_line(trimmed: &str) -> Result<ErAttribute, String> {
    let Some(ci) = find_unquoted_colon(trimmed) else {
        return Err(format!(
            "syntax error: expected `name: type` in entity attribute, got `{}`",
            trimmed.chars().take(40).collect::<String>()
        ));
    };
    let name = trimmed[..ci].trim();
    if !is_ident(name) {
        return Err(format!("invalid attribute name `{name}`"));
    }
    let rest = trimmed[ci + 1..].trim();
    let tokens = tokenize_ws_quoted(rest);
    if tokens.is_empty() {
        return Err("expected a type after `:`".to_string());
    }
    let type_name = tokens[0].clone();
    let mut keys = Vec::new();
    let mut comment = None;
    for t in &tokens[1..] {
        if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
            if comment.is_some() {
                return Err("multiple comments on one attribute line".to_string());
            }
            comment = Some(t[1..t.len() - 1].to_string());
        } else if matches!(t.as_str(), "PK" | "FK" | "UK") {
            keys.push(t.clone());
        } else {
            return Err(format!(
                "syntax error: unrecognised token `{t}` in entity attribute"
            ));
        }
    }
    Ok(ErAttribute::new(type_name, name, keys, comment))
}

/// Parse an `er <name> { ... }` diagram from `src`. The caller has already
/// confirmed the header keyword is `er`.
///
/// Internally this scanner works in byte offsets (needed for correct UTF-8
/// slicing); errors are converted to character offsets before being returned,
/// matching the convention the rest of `kozue-dsl` (chumsky) uses.
pub fn parse(src: &str) -> Result<Diagram, Vec<CompileError>> {
    parse_bytes(src).map_err(|errs| crate::class_dsl::to_char_spans(src, errs))
}

fn parse_bytes(src: &str) -> Result<Diagram, Vec<CompileError>> {
    let Some((kw, kw_span)) = crate::peek_header_keyword(src) else {
        return Err(vec![err("expected `er` keyword", 0..src.len().max(1))]);
    };
    if kw != "er" {
        return Err(vec![err(
            format!("expected `er` keyword, got `{kw}`"),
            kw_span,
        )]);
    }
    let mut idx = kw_span.end;
    idx = skip_ws_comments(src, idx);
    let Some((_name, name_span)) = peek_ident(src, idx) else {
        return Err(vec![err(
            "expected a diagram name after `er`",
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

    let mut diagram = ErDiagram::new();
    let ensure_entity = |diagram: &mut ErDiagram, id: &str| {
        if !diagram.entities.contains_key(id) {
            diagram.entities.insert(
                id.to_string(),
                ErEntity::new(id.to_string(), id.to_string()),
            );
        }
    };

    let mut i = 0usize;
    while i < lines.len() {
        let (offset, line) = lines[i];
        let span = offset..offset + line.len();

        if let Some(stripped) = line.strip_prefix("entity").and_then(|r| {
            if r.is_empty() || r.starts_with(|c: char| c.is_whitespace()) {
                Some(r.trim())
            } else {
                None
            }
        }) {
            // Three shapes: inline `entity Foo { a; b }` (single line,
            // `;`-separated attributes) or a multi-line block
            // `entity Foo {` ... `}`. A bare `entity Foo` (no block) is not
            // meaningful for an ER entity (it would have no attributes) and
            // is rejected explicitly.
            let (name, inline_body) = if let Some(open_idx) = stripped.find('{') {
                if let Some(inner) = stripped[open_idx + 1..].strip_suffix('}') {
                    (stripped[..open_idx].trim(), Some(inner.trim()))
                } else if stripped.trim_end().ends_with('{') {
                    (stripped[..open_idx].trim(), None)
                } else {
                    errors.push(err(
                        format!(
                            "syntax error: malformed entity body `{}`",
                            line.chars().take(40).collect::<String>()
                        ),
                        span,
                    ));
                    i += 1;
                    continue;
                }
            } else {
                errors.push(err("expected `entity <name> { ... }` block", span));
                i += 1;
                continue;
            };
            if !is_ident(name) {
                errors.push(err(
                    format!(
                        "expected an entity identifier, got `{}`",
                        name.chars().take(40).collect::<String>()
                    ),
                    span,
                ));
                i += 1;
                continue;
            }
            ensure_entity(&mut diagram, name);

            if let Some(inner) = inline_body {
                for member in inner.split(';') {
                    let member = member.trim();
                    if member.is_empty() {
                        continue;
                    }
                    match parse_attr_line(member) {
                        Ok(attr) => diagram.entities[name].attributes.push(attr),
                        Err(msg) => errors.push(err(msg, span.clone())),
                    }
                }
                i += 1;
                continue;
            }

            i += 1;
            loop {
                if i >= lines.len() {
                    errors.push(err(
                        format!("unterminated `{name} {{ ... }}` entity block (missing `}}`)"),
                        span.clone(),
                    ));
                    break;
                }
                let (moff, mline) = lines[i];
                if mline.trim() == "}" {
                    i += 1;
                    break;
                }
                match parse_attr_line(mline) {
                    Ok(attr) => diagram.entities[name].attributes.push(attr),
                    Err(msg) => errors.push(err(msg, moff..moff + mline.len())),
                }
                i += 1;
            }
            continue;
        }

        if let Some(result) = try_parse_relation(line) {
            match result {
                Ok(rel) => {
                    if rel.from == rel.to {
                        errors.push(err(
                            format!(
                                "self relations are not supported in ER diagrams (`{}` -> `{}`)",
                                rel.from, rel.to
                            ),
                            span,
                        ));
                    } else {
                        ensure_entity(&mut diagram, &rel.from);
                        ensure_entity(&mut diagram, &rel.to);
                        diagram.relations.push(ErRelation::new(
                            rel.from,
                            rel.to,
                            rel.from_marker,
                            rel.to_marker,
                            rel.label,
                            rel.line,
                        ));
                    }
                }
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

    if errors.is_empty() {
        Ok(Diagram::Er(diagram))
    } else {
        Err(errors)
    }
}

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
