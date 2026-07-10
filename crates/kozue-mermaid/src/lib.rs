//! Mermaid-syntax frontend for kozue.
//!
//! Parses a subset of Mermaid flowchart and sequence diagrams into the same
//! [`kozue_ir::Diagram`] semantic IR used by the native kozue DSL. Layout and
//! rendering are handled by the existing kozue pipeline unchanged.
//!
//! # Supported syntax
//!
//! ## Flowchart
//! ```text
//! flowchart TD
//!   A[開始] --> B[処理]
//!   B -->|OK| C[終了]
//!   B -->|NG| D[エラー]
//!   C --> E
//!   D --> E[完了]
//! ```
//!
//! ## Sequence diagram
//! ```text
//! sequenceDiagram
//!   participant A as Alice
//!   participant B
//!   A->>B: こんにちは
//!   B-->>A: 返事
//! ```
//!
//! # Compatibility notes
//!
//! - Node shapes: `[label]` (rectangular) and `(label)` (rounded) both map to
//!   `NodeKind::Default`; shape differences are not rendered.
//! - Sequence open arrows `->` and `-->` map to `ArrowType::Triangle` with the
//!   same solid/dashed line style as `-->>` / `->>`.
//! - Unsupported features (RL/BT direction, Note, loop, alt, subgraph, classDef,
//!   style, etc.) are reported as positioned "unsupported" errors rather than
//!   crashing or silently ignoring.

pub mod features;

use std::ops::Range;

use ariadne::{Label, Report, ReportKind, Source};
use indexmap::IndexMap;
use kozue_ir::{
    ArrowType, Diagram, Direction, Edge, GraphDiagram, LineStyle, Message, Node, Participant,
    SequenceDiagram, SequenceItem,
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

/// Parse Mermaid source text into a semantic [`Diagram`].
///
/// Returns `Ok(diagram)` on success, or `Err(diagnostics)` where diagnostics
/// is a non-empty list of errors (all errors from the whole source are
/// collected before returning, following the same convention as `kozue-dsl`).
pub fn parse(source: &str) -> Result<Diagram, Vec<Diagnostic>> {
    let mut errors: Vec<Diagnostic> = Vec::new();

    // Strip UTF-8 BOM if present.
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);

    // Tokenise into logical lines (strip comments, empty lines, leading whitespace).
    let lines: Vec<(usize, &str)> = logical_lines(source);

    if lines.is_empty() {
        errors.push(Diagnostic::new(
            "empty diagram: expected `flowchart` or `sequenceDiagram` header",
            0..source.len().max(1),
        ));
        return Err(errors);
    }

    let (header_offset, header_line) = lines[0];
    let header_trimmed = header_line.trim();

    // Detect diagram kind from the header line.
    if let Some(rest) = strip_keyword_ci(header_trimmed, "sequenceDiagram") {
        if !rest.trim().is_empty() {
            errors.push(Diagnostic::new(
                "unexpected tokens after `sequenceDiagram`; the header must be on its own line",
                header_offset..header_offset + header_line.len(),
            ));
        }
        parse_sequence(&lines[1..], source, &mut errors)
    } else if let Some(rest) = strip_keyword_ci(header_trimmed, "flowchart")
        .or_else(|| strip_keyword_ci(header_trimmed, "graph"))
    {
        let dir_str = rest.trim();
        let direction = match dir_str.to_ascii_uppercase().as_str() {
            "TD" | "TB" => Direction::Down,
            "LR" => Direction::Right,
            "RL" => {
                errors.push(Diagnostic::new(
                    "unsupported: direction RL (kozue does not support this yet)",
                    header_offset..header_offset + header_line.len(),
                ));
                Direction::Down // keep going to collect more errors
            }
            "BT" => {
                errors.push(Diagnostic::new(
                    "unsupported: direction BT (kozue does not support this yet)",
                    header_offset..header_offset + header_line.len(),
                ));
                Direction::Down
            }
            "" => {
                // Mermaid allows omitting direction; default to TD.
                Direction::Down
            }
            _ => {
                errors.push(Diagnostic::new(
                    format!("unknown flowchart direction `{dir_str}`; expected TD, TB, or LR"),
                    header_offset..header_offset + header_line.len(),
                ));
                Direction::Down
            }
        };
        parse_flowchart(&lines[1..], source, direction, &mut errors)
    } else {
        errors.push(Diagnostic::new(
            format!(
                "unrecognised diagram header `{}`; expected `flowchart`, `graph`, or `sequenceDiagram`",
                header_trimmed.chars().take(40).collect::<String>()
            ),
            header_offset..header_offset + header_line.len(),
        ));
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
// Flowchart parser
// ---------------------------------------------------------------------------

fn parse_flowchart(
    lines: &[(usize, &str)],
    _source: &str,
    direction: Direction,
    errors: &mut Vec<Diagnostic>,
) -> Result<Diagram, Vec<Diagnostic>> {
    // nodes: id -> (label, span of first declaration)
    let mut node_labels: IndexMap<String, String> = IndexMap::new();
    // Raw edges to process after scanning all lines.
    struct RawEdge {
        from: String,
        to: String,
        label: Option<String>,
        arrow: ArrowType,
        span: Range<usize>,
    }
    let mut raw_edges: Vec<RawEdge> = Vec::new();

    // Helper: register a node (first-declared label wins).
    let mut ensure_node = |id: &str, label: Option<&str>| {
        if !node_labels.contains_key(id) {
            let lbl = label.unwrap_or(id).to_string();
            node_labels.insert(id.to_string(), lbl);
        }
        // If node already exists and a different label is given, silently ignore
        // (Mermaid: first occurrence wins).
    };

    for &(offset, line) in lines {
        let trimmed = line.trim();
        let line_end = offset + line.len();
        let span = offset..line_end;

        // Skip subgraph / end blocks (unsupported).
        if trimmed.starts_with("subgraph") {
            errors.push(Diagnostic::new(
                "unsupported: subgraph (kozue does not support this yet)",
                span,
            ));
            continue;
        }
        if trimmed == "end" {
            // silently skip — it closes an unsupported subgraph
            continue;
        }

        // Check for classDef / class / style / linkStyle (unsupported styling).
        if trimmed.starts_with("classDef")
            || trimmed.starts_with("class ")
            || trimmed.starts_with("style ")
            || trimmed.starts_with("linkStyle")
            || trimmed.starts_with("click ")
        {
            let feature = trimmed.split_whitespace().next().unwrap_or("style");
            errors.push(Diagnostic::new(
                format!("unsupported: {feature} (kozue does not support this yet)"),
                span,
            ));
            continue;
        }

        // Semicolon separator (unsupported — we only handle newline-separated stmts).
        if trimmed.contains(';') {
            errors.push(Diagnostic::new(
                "unsupported: semicolon statement separator; use newlines instead (kozue does not support this yet)",
                span,
            ));
            continue;
        }

        // Try to parse as an edge line (including chain notation A --> B --> C).
        // We support:
        //   A --> B            (arrow)
        //   A --- B            (no arrow)
        //   A -->|label| B     (arrow + pipe label)
        //   A -- label --> B   (arrow + space label)
        //   A -- label --- B   (no-arrow + space label)
        //   A --> B --> C      (chain — generates multiple edges)
        // Each endpoint may optionally have [label] or (label).
        if let Some(chain) = try_parse_edge_chain(trimmed, offset) {
            match chain {
                Ok(edges) => {
                    for (from_id, from_label, to_id, to_label, edge_label, arrow) in edges {
                        ensure_node(&from_id, from_label.as_deref());
                        ensure_node(&to_id, to_label.as_deref());
                        raw_edges.push(RawEdge {
                            from: from_id,
                            to: to_id,
                            label: edge_label,
                            arrow,
                            span: span.clone(),
                        });
                    }
                }
                Err(msg) => {
                    errors.push(Diagnostic::new(msg, span));
                }
            }
            continue;
        }

        // Detect stadium `A([label])` and circle `A((label))` shapes before the generic
        // node-decl path, so we can emit an explicit "unsupported" error instead of
        // a confusing generic one.
        if let Some((_, rest_after_id)) = split_id(trimmed) {
            let rest_trimmed = rest_after_id.trim_start();
            if rest_trimmed.starts_with("([") || rest_trimmed.starts_with("((") {
                errors.push(Diagnostic::new(
                    "unsupported: stadium/circle node shape (`([…])` / `((…))`); kozue does not support this yet",
                    span,
                ));
                continue;
            }
        }

        // Try to parse as a standalone node declaration: `A[label]`, `A(label)`, or bare `A`.
        if let Some((id, label)) = try_parse_node_decl(trimmed) {
            ensure_node(&id, label.as_deref());
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

    // Build GraphDiagram.
    let mut graph = GraphDiagram::new(direction);
    for (id, label) in &node_labels {
        graph
            .nodes
            .insert(id.clone(), Node::new(id.clone(), label.clone()));
    }

    for re in &raw_edges {
        // Self-loop check.
        if re.from == re.to {
            errors.push(Diagnostic::new(
                format!(
                    "self-loops are not supported in flowchart diagrams (edge `{}` --> `{}`)",
                    re.from, re.to
                ),
                re.span.clone(),
            ));
            continue;
        }
        graph.edges.push(Edge::new(
            re.from.clone(),
            re.to.clone(),
            re.label.clone(),
            re.arrow,
        ));
    }

    if errors.is_empty() {
        Ok(Diagram::Graph(graph))
    } else {
        Err(errors.clone())
    }
}

// ---------------------------------------------------------------------------
// Sequence parser
// ---------------------------------------------------------------------------

fn parse_sequence(
    lines: &[(usize, &str)],
    _source: &str,
    errors: &mut Vec<Diagnostic>,
) -> Result<Diagram, Vec<Diagnostic>> {
    let mut seq = SequenceDiagram::new();

    struct RawMsg {
        from: String,
        to: String,
        label: Option<String>,
        line_style: LineStyle,
        arrow: ArrowType,
        #[allow(dead_code)]
        span: Range<usize>,
    }
    let mut messages: Vec<RawMsg> = Vec::new();

    let ensure_participant = |seq: &mut SequenceDiagram, id: &str, label: Option<&str>| {
        if !seq.participants.contains_key(id) {
            let lbl = label.unwrap_or(id).to_string();
            seq.participants
                .insert(id.to_string(), Participant::new(id.to_string(), lbl));
        }
    };

    for &(offset, line) in lines {
        let trimmed = line.trim();
        let line_end = offset + line.len();
        let span = offset..line_end;

        // Semicolon check.
        if trimmed.contains(';') {
            errors.push(Diagnostic::new(
                "unsupported: semicolon statement separator; use newlines instead (kozue does not support this yet)",
                span,
            ));
            continue;
        }

        // Unsupported Mermaid keywords in sequence diagrams.
        // Each entry is the keyword with a trailing space or end-of-string boundary
        // so "par" doesn't accidentally match "participant".
        let unsupported_kw: &[&str] = &[
            "Note",
            "note",
            "loop",
            "alt",
            "else",
            "opt",
            "par",
            "break",
            "activate",
            "deactivate",
            "rect",
            "autonumber",
            "title",
            "accTitle",
            "accDescr",
        ];
        let mut found_unsupported = false;
        for kw in unsupported_kw {
            // Word-boundary match: keyword must be followed by whitespace or end-of-string.
            if trimmed == *kw
                || (trimmed.starts_with(kw)
                    && trimmed[kw.len()..].starts_with(|c: char| c.is_ascii_whitespace()))
            {
                let feature = trimmed.split_whitespace().next().unwrap_or(kw);
                errors.push(Diagnostic::new(
                    format!("unsupported: {feature} (kozue does not support this yet)"),
                    span.clone(),
                ));
                found_unsupported = true;
                break;
            }
        }
        if found_unsupported {
            continue;
        }

        // `end` closes loop/alt/opt blocks — silently skip.
        if trimmed == "end" {
            continue;
        }

        // participant declaration.
        if trimmed.starts_with("participant ") || trimmed.starts_with("actor ") {
            let rest = if let Some(r) = trimmed.strip_prefix("participant ") {
                r.trim()
            } else {
                trimmed.strip_prefix("actor ").unwrap_or("").trim()
            };
            // `participant X as Label` or `participant X`
            let (id, label) = if let Some(idx) = find_keyword_boundary(rest, " as ") {
                let id = rest[..idx].trim().to_string();
                let label = rest[idx + 4..].trim().to_string();
                (id, Some(label))
            } else {
                (rest.to_string(), None)
            };
            if id.is_empty() {
                errors.push(Diagnostic::new("expected participant identifier", span));
                continue;
            }
            ensure_participant(&mut seq, &id, label.as_deref());
            continue;
        }

        // Message arrow lines: from->>to: label  /  from-->>to: label  etc.
        if let Some(msg) = try_parse_seq_message(trimmed, offset) {
            match msg {
                Ok((from, to, label, line_style, arrow)) => {
                    // Auto-declare participants.
                    ensure_participant(&mut seq, &from, None);
                    ensure_participant(&mut seq, &to, None);
                    messages.push(RawMsg {
                        from,
                        to,
                        label,
                        line_style,
                        arrow,
                        span,
                    });
                }
                Err(msg_err) => {
                    errors.push(Diagnostic::new(msg_err, span));
                }
            }
            continue;
        }

        // Unrecognised.
        errors.push(Diagnostic::new(
            format!(
                "syntax error: unrecognised statement `{}`",
                trimmed.chars().take(40).collect::<String>()
            ),
            span,
        ));
    }

    for rm in messages {
        seq.items.push(SequenceItem::Message(Message::new(
            rm.from,
            rm.to,
            rm.label,
            rm.line_style,
            rm.arrow,
        )));
    }

    if errors.is_empty() {
        Ok(Diagram::Sequence(seq))
    } else {
        Err(errors.clone())
    }
}

// ---------------------------------------------------------------------------
// Line tokenisation helpers
// ---------------------------------------------------------------------------

/// Collect non-empty, non-comment lines with their byte offsets.
///
/// Each element is `(byte_offset_of_line_start, raw_line_without_newline)`.
/// Comment lines (starting with `%%` after stripping whitespace) are excluded.
fn logical_lines(source: &str) -> Vec<(usize, &str)> {
    let mut result = Vec::new();
    let mut offset = 0usize;
    for raw in source.split('\n') {
        let trimmed = raw.trim();
        if !trimmed.is_empty() && !trimmed.starts_with("%%") {
            result.push((offset, raw));
        }
        offset += raw.len() + 1; // +1 for the '\n' that was consumed by split
    }
    result
}

/// Strip a keyword prefix (case-insensitive) and return the rest, or None.
fn strip_keyword_ci<'a>(s: &'a str, keyword: &str) -> Option<&'a str> {
    if (s.len() >= keyword.len()
        && s[..keyword.len()].eq_ignore_ascii_case(keyword)
        // Must be followed by whitespace or end-of-string (word boundary).
        && (s.len() == keyword.len()
            || s[keyword.len()..].starts_with(|c: char| c.is_ascii_whitespace() || c == '\0')))
        || s.eq_ignore_ascii_case(keyword)
    {
        Some(&s[keyword.len()..])
    } else {
        None
    }
}

/// Find `needle` within `haystack` respecting word-boundary (space before and after is part of needle).
fn find_keyword_boundary(haystack: &str, needle: &str) -> Option<usize> {
    haystack.find(needle)
}

// ---------------------------------------------------------------------------
// Node / edge line parsers
// ---------------------------------------------------------------------------

/// (from_id, from_label, to_id, to_label, edge_label, arrow)
type EdgeParseResult = (
    String,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    ArrowType,
);

/// (from, to, label, line_style, arrow)
type SeqMsgResult = (String, String, Option<String>, LineStyle, ArrowType);

/// (to_id, to_label, edge_label, arrow, remainder) — result of one edge segment parse.
type SegmentResult<'a> = (String, Option<String>, Option<String>, ArrowType, &'a str);

/// Try to parse a node identifier possibly followed by a shape label: `A[label]`, `A(label)`, or `A`.
///
/// Returns `Some((id, Option<label>))` if the line is a valid standalone node declaration.
/// Returns `None` if the line cannot be a node declaration (e.g. it looks like an edge).
fn try_parse_node_decl(line: &str) -> Option<(String, Option<String>)> {
    // If the line contains any arrow operator it's an edge, not a node.
    if line.contains("-->")
        || line.contains("---")
        || line.contains("->>")
        || line.contains("-->>")
        || line.contains("->")
        || line.contains("-->")
    {
        return None;
    }
    let (id, rest) = split_id(line)?;
    let rest = rest.trim();
    if rest.is_empty() {
        return Some((id, None));
    }
    if rest.starts_with('[') {
        let label = extract_bracket(rest, '[', ']')?;
        return Some((id, Some(label)));
    }
    if rest.starts_with('(') {
        let label = extract_bracket(rest, '(', ')')?;
        return Some((id, Some(label)));
    }
    None
}

/// Parse one edge operator+target segment from `rest`, returning
/// `(to_id, to_label, edge_label, arrow, remainder_after_to_node)` or None/Err.
fn parse_one_edge_segment(rest: &str) -> Option<Result<SegmentResult<'_>, String>> {
    let rest = rest.trim_start();

    // Check for multi-target `&` — must error explicitly.
    if rest.starts_with('&') {
        return Some(Err(
            "unsupported: multi-target edge (`&`); split into separate edge lines instead"
                .to_string(),
        ));
    }

    // Try `-->|label|` first (pipe-label form with arrow).
    if let Some(rest2) = rest.strip_prefix("-->") {
        let rest2 = rest2.trim_start();
        if rest2.starts_with('|') {
            // `-->|label| to_node`
            let (edge_label, rest3) = extract_pipe_label(rest2)?;
            let rest3 = rest3.trim_start();
            // Strictly validate: rest3 must start with a valid node identifier.
            // parse_node_with_label will return None if it cannot parse a node.
            let (to_id, to_label, after) = match parse_node_with_label(rest3) {
                Some(r) => r,
                None => {
                    return Some(Err(format!(
                        "syntax error: expected node identifier after `-->|{}|`, got `{}`",
                        edge_label,
                        rest3.chars().take(20).collect::<String>()
                    )));
                }
            };
            let after = after.trim_start();
            // Check for multi-target `&` after the to-node.
            if after.starts_with('&') {
                return Some(Err(
                    "unsupported: multi-target edge (`&`); split into separate edge lines instead"
                        .to_string(),
                ));
            }
            return Some(Ok((
                to_id,
                to_label,
                Some(edge_label),
                ArrowType::Triangle,
                after,
            )));
        } else {
            // Check for `&` (multi-target) before to-node.
            if rest2.trim_start().starts_with('&') {
                return Some(Err(
                    "unsupported: multi-target edge (`&`); split into separate edge lines instead"
                        .to_string(),
                ));
            }
            // `-->  to_node` (no label)
            let (to_id, to_label, after) = parse_node_with_label(rest2)?;
            let after = after.trim_start();
            if after.starts_with('&') {
                return Some(Err(
                    "unsupported: multi-target edge (`&`); split into separate edge lines instead"
                        .to_string(),
                ));
            }
            return Some(Ok((to_id, to_label, None, ArrowType::Triangle, after)));
        }
    }

    // Try `---` (no-arrow line).
    if let Some(rest2) = rest.strip_prefix("---") {
        let rest2 = rest2.trim_start();
        if rest2.starts_with('&') {
            return Some(Err(
                "unsupported: multi-target edge (`&`); split into separate edge lines instead"
                    .to_string(),
            ));
        }
        let (to_id, to_label, after) = parse_node_with_label(rest2)?;
        let after = after.trim_start();
        if after.starts_with('&') {
            return Some(Err(
                "unsupported: multi-target edge (`&`); split into separate edge lines instead"
                    .to_string(),
            ));
        }
        return Some(Ok((to_id, to_label, None, ArrowType::None, after)));
    }

    // Try `-- label -->` form (space label with arrow).
    if let Some(rest2) = rest.strip_prefix("-- ") {
        if let Some(arrow_idx) = rest2.find("-->") {
            let label = rest2[..arrow_idx].trim().to_string();
            let rest3 = rest2[arrow_idx + 3..].trim();
            if rest3.starts_with('&') {
                return Some(Err(
                    "unsupported: multi-target edge (`&`); split into separate edge lines instead"
                        .to_string(),
                ));
            }
            let (to_id, to_label, after) = parse_node_with_label(rest3)?;
            let after = after.trim_start();
            if after.starts_with('&') {
                return Some(Err(
                    "unsupported: multi-target edge (`&`); split into separate edge lines instead"
                        .to_string(),
                ));
            }
            let edge_label = if label.is_empty() { None } else { Some(label) };
            return Some(Ok((
                to_id,
                to_label,
                edge_label,
                ArrowType::Triangle,
                after,
            )));
        }
        if let Some(arrow_idx) = rest2.find("---") {
            let label = rest2[..arrow_idx].trim().to_string();
            let rest3 = rest2[arrow_idx + 3..].trim();
            if rest3.starts_with('&') {
                return Some(Err(
                    "unsupported: multi-target edge (`&`); split into separate edge lines instead"
                        .to_string(),
                ));
            }
            let (to_id, to_label, after) = parse_node_with_label(rest3)?;
            let after = after.trim_start();
            if after.starts_with('&') {
                return Some(Err(
                    "unsupported: multi-target edge (`&`); split into separate edge lines instead"
                        .to_string(),
                ));
            }
            let edge_label = if label.is_empty() { None } else { Some(label) };
            return Some(Ok((to_id, to_label, edge_label, ArrowType::None, after)));
        }
    }

    None
}

/// Try to parse a flowchart edge line, returning all edges (for chain notation).
///
/// `A --> B --> C --> D` generates three edges. The function returns `None` if
/// the line doesn't start with a recognisable edge pattern, or
/// `Some(Err(msg))` for a detected-but-invalid edge, or
/// `Some(Ok(vec))` with one or more edges on success.
fn try_parse_edge_chain(
    line: &str,
    _offset: usize,
) -> Option<Result<Vec<EdgeParseResult>, String>> {
    let (first_id, first_label, rest) = parse_node_with_label(line)?;
    let rest = rest.trim_start();

    // Must look like an edge (starts with an operator).
    if !rest.starts_with("-->") && !rest.starts_with("---") && !rest.starts_with("-- ") {
        return None;
    }

    let mut results: Vec<EdgeParseResult> = Vec::new();
    let mut from_id = first_id;
    let mut from_label = first_label;
    let mut current_rest = rest;

    loop {
        match parse_one_edge_segment(current_rest) {
            None => {
                // No operator recognised — if we already parsed at least one edge,
                // any non-empty remainder is an error.
                if current_rest.is_empty() {
                    break;
                }
                return Some(Err(format!(
                    "syntax error: unexpected tokens in edge: `{}`",
                    current_rest.chars().take(40).collect::<String>()
                )));
            }
            Some(Err(msg)) => return Some(Err(msg)),
            Some(Ok((to_id, to_label, edge_label, arrow, remainder))) => {
                results.push((
                    from_id.clone(),
                    from_label.clone(),
                    to_id.clone(),
                    to_label.clone(),
                    edge_label,
                    arrow,
                ));
                from_id = to_id;
                from_label = to_label;
                current_rest = remainder.trim_start();
                if current_rest.is_empty() {
                    break;
                }
                // If remainder doesn't start with an operator, it's an error.
                if !current_rest.starts_with("-->")
                    && !current_rest.starts_with("---")
                    && !current_rest.starts_with("-- ")
                {
                    return Some(Err(format!(
                        "syntax error: unexpected tokens after edge: `{}`",
                        current_rest.chars().take(40).collect::<String>()
                    )));
                }
            }
        }
    }

    if results.is_empty() {
        None
    } else {
        Some(Ok(results))
    }
}

/// Parse a node reference at the start of `s`, which may have an optional shape
/// label (`[label]` or `(label)`). Returns `(id, Option<label>, rest)` or None.
///
/// Stadium/circle shapes (`([` or `((`) are NOT parsed here; they are detected and
/// rejected by the caller (try_parse_node_decl_checked / try_parse_edge_chain) so
/// that a clear "unsupported" error is reported instead of silently mangling the label.
fn parse_node_with_label(s: &str) -> Option<(String, Option<String>, &str)> {
    let (id, rest) = split_id(s)?;
    let rest = rest.trim_start();
    if rest.starts_with('[') {
        let label = extract_bracket(rest, '[', ']')?;
        // Advance rest past the bracket expression.
        let close = rest.find(']')?;
        let after = &rest[close + 1..];
        return Some((id, Some(label), after));
    }
    // Reject stadium `([` and circle `((` shapes — do not attempt to parse.
    if rest.starts_with("([") || rest.starts_with("((") {
        return None;
    }
    if rest.starts_with('(') {
        let label = extract_bracket(rest, '(', ')')?;
        let close = rest.find(')')?;
        let after = &rest[close + 1..];
        return Some((id, Some(label), after));
    }
    Some((id, None, rest))
}

/// Split off a leading Mermaid identifier (ASCII alphanumeric + underscore).
/// Returns `(id, rest_of_str)` or `None` if no identifier found.
fn split_id(s: &str) -> Option<(String, &str)> {
    let s = s.trim_start();
    if s.is_empty() {
        return None;
    }
    // Mermaid IDs: letters, digits, underscore, hyphen (in some contexts).
    // We accept alphanumeric + underscore as a safe conservative subset.
    let end = s
        .char_indices()
        .find(|&(_, c)| !c.is_alphanumeric() && c != '_')
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    if end == 0 {
        return None;
    }
    let id = s[..end].to_string();
    Some((id, &s[end..]))
}

/// Extract the content between open/close bracket chars, handling nested brackets.
fn extract_bracket(s: &str, open: char, close: char) -> Option<String> {
    let s = s.trim_start();
    if !s.starts_with(open) {
        return None;
    }
    let mut depth = 0usize;
    let mut content = String::new();
    let mut chars = s.chars().peekable();
    let mut started = false;
    for c in chars.by_ref() {
        if c == open {
            if started {
                depth += 1;
                content.push(c);
            } else {
                started = true;
                depth = 1;
            }
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                break;
            }
            content.push(c);
        } else {
            content.push(c);
        }
    }
    if depth == 0 && started {
        Some(content)
    } else {
        None
    }
}

/// Extract `|label|` from the front of `s`, returning `(label, rest)`.
fn extract_pipe_label(s: &str) -> Option<(String, &str)> {
    let s = s.trim_start();
    if !s.starts_with('|') {
        return None;
    }
    let s = &s[1..]; // skip opening |
    let end = s.find('|')?;
    let label = s[..end].to_string();
    let rest = &s[end + 1..];
    Some((label, rest))
}

// ---------------------------------------------------------------------------
// Sequence message parser
// ---------------------------------------------------------------------------

/// Try to parse a sequence diagram message line.
///
/// Supported arrow forms and their mapping:
/// | Mermaid  | LineStyle | ArrowType |
/// |----------|-----------|-----------|
/// | `->>` | Solid | Triangle |
/// | `-->>` | Dashed | Triangle |
/// | `->` | Solid | Triangle (compat note: open-arrow mapped to Triangle) |
/// | `-->` | Dashed | Triangle (compat note: open-arrow mapped to Triangle) |
///
/// Format: `from ARROW to: label` or `from ARROW to`
fn try_parse_seq_message(line: &str, _offset: usize) -> Option<Result<SeqMsgResult, String>> {
    // Try each arrow form, longest first to avoid prefix ambiguity.
    // Order matters: `-->>` before `-->`, `->>` before `->`.
    let arrow_forms: &[(&str, LineStyle, ArrowType)] = &[
        ("-->>", LineStyle::Dashed, ArrowType::Triangle),
        ("-->", LineStyle::Dashed, ArrowType::Triangle),
        ("->>", LineStyle::Solid, ArrowType::Triangle),
        ("->", LineStyle::Solid, ArrowType::Triangle),
    ];

    for &(arrow_str, line_style, arrow) in arrow_forms {
        if let Some(idx) = line.find(arrow_str) {
            // Verify from-part is a valid identifier.
            let from_part = line[..idx].trim();
            let after = &line[idx + arrow_str.len()..];

            let (from, _) = split_id(from_part)?;
            if from != from_part {
                continue; // from-part has extra characters, try next arrow form
            }

            // Parse `to: label` or just `to`.
            let after = after.trim_start();
            let (to, label) = if let Some(colon_idx) = after.find(':') {
                let to_part = after[..colon_idx].trim();
                let (to, _) = split_id(to_part)?;
                if to != to_part {
                    continue;
                }
                let label = after[colon_idx + 1..].trim().to_string();
                let label = if label.is_empty() { None } else { Some(label) };
                (to, label)
            } else {
                let (to, rest) = split_id(after)?;
                if !rest.trim().is_empty() {
                    continue; // unexpected trailing content
                }
                (to, None)
            };

            return Some(Ok((from, to, label, line_style, arrow)));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use kozue_ir::{ArrowType, Direction, LineStyle};

    // -----------------------------------------------------------------------
    // Flowchart tests
    // -----------------------------------------------------------------------

    #[test]
    fn flowchart_td_basic() {
        let src = "flowchart TD\n  A[開始] --> B[処理]\n  B --> C[終了]\n";
        let d = parse(src).expect("should parse");
        let Diagram::Graph(g) = d else {
            panic!("expected Graph")
        };
        assert_eq!(g.direction, Direction::Down);
        assert_eq!(g.nodes.len(), 3);
        assert_eq!(g.nodes["A"].label, "開始");
        assert_eq!(g.nodes["B"].label, "処理");
        assert_eq!(g.nodes["C"].label, "終了");
        assert_eq!(g.edges.len(), 2);
    }

    #[test]
    fn flowchart_tb_is_down() {
        let src = "flowchart TB\n  A --> B\n";
        let d = parse(src).expect("should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.direction, Direction::Down);
    }

    #[test]
    fn flowchart_lr() {
        let src = "flowchart LR\n  A --> B\n";
        let d = parse(src).expect("should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.direction, Direction::Right);
    }

    #[test]
    fn graph_keyword_accepted() {
        let src = "graph TD\n  A --> B\n";
        let d = parse(src).expect("should parse `graph` keyword");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.direction, Direction::Down);
    }

    #[test]
    fn first_label_wins_no_overwrite() {
        // Mermaid: first occurrence of a node's label wins.
        let src = "flowchart TD\n  A[First] --> B\n  A[Second] --> C\n";
        let d = parse(src).expect("should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.nodes["A"].label, "First", "first label must win");
    }

    #[test]
    fn bare_node_autodeclared_with_id_as_label() {
        let src = "flowchart TD\n  A --> B\n";
        let d = parse(src).expect("should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.nodes["A"].label, "A");
        assert_eq!(g.nodes["B"].label, "B");
    }

    #[test]
    fn pipe_label_on_edge() {
        let src = "flowchart TD\n  A -->|OK| B\n";
        let d = parse(src).expect("should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.edges[0].label.as_deref(), Some("OK"));
    }

    #[test]
    fn space_label_on_edge() {
        let src = "flowchart TD\n  A -- yes --> B\n";
        let d = parse(src).expect("should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.edges[0].label.as_deref(), Some("yes"));
    }

    #[test]
    fn no_arrow_edge() {
        let src = "flowchart TD\n  A --- B\n";
        let d = parse(src).expect("should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.edges[0].arrow, ArrowType::None);
    }

    #[test]
    fn self_loop_in_flowchart_is_error() {
        let src = "flowchart TD\n  A --> A\n";
        let errs = parse(src).expect_err("self-loop should be error");
        assert!(
            errs.iter()
                .any(|e| e.message.contains("self-loops are not supported")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn direction_rl_is_unsupported() {
        let src = "flowchart RL\n  A --> B\n";
        let errs = parse(src).expect_err("RL should be unsupported");
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unsupported") && e.message.contains("RL")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn direction_bt_is_unsupported() {
        let src = "flowchart BT\n  A --> B\n";
        let errs = parse(src).expect_err("BT should be unsupported");
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unsupported") && e.message.contains("BT")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn semicolon_in_flowchart_is_unsupported() {
        let src = "flowchart TD\n  A --> B; B --> C\n";
        let errs = parse(src).expect_err("semicolon should be unsupported");
        assert!(
            errs.iter().any(|e| e.message.contains("semicolon")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn comment_lines_are_ignored() {
        let src = "flowchart TD\n  %% this is a comment\n  A --> B\n";
        let d = parse(src).expect("comments should be ignored");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.nodes.len(), 2);
    }

    #[test]
    fn rounded_node_shape_maps_to_default() {
        let src = "flowchart TD\n  A(丸形) --> B\n";
        let d = parse(src).expect("round shape should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.nodes["A"].label, "丸形");
        assert_eq!(g.nodes["A"].kind, kozue_ir::NodeKind::Default);
    }

    // -----------------------------------------------------------------------
    // Sequence tests
    // -----------------------------------------------------------------------

    #[test]
    fn sequence_basic() {
        let src = "sequenceDiagram\n  participant A as Alice\n  participant B\n  A->>B: こんにちは\n  B-->>A: 返事\n";
        let d = parse(src).expect("should parse");
        let Diagram::Sequence(s) = d else { panic!() };
        assert_eq!(s.participants.len(), 2);
        assert_eq!(s.participants["A"].label, "Alice");
        assert_eq!(s.participants["B"].label, "B");
        assert_eq!(s.items.len(), 2);
        let SequenceItem::Message(ref m0) = s.items[0] else {
            panic!()
        };
        assert_eq!(m0.line, LineStyle::Solid);
        assert_eq!(m0.label.as_deref(), Some("こんにちは"));
        let SequenceItem::Message(ref m1) = s.items[1] else {
            panic!()
        };
        assert_eq!(m1.line, LineStyle::Dashed);
    }

    #[test]
    fn sequence_autodeclare_participant() {
        // Participants not declared via `participant` are auto-declared on first message.
        let src = "sequenceDiagram\n  A->>B: hello\n";
        let d = parse(src).expect("should parse");
        let Diagram::Sequence(s) = d else { panic!() };
        assert_eq!(s.participants.len(), 2);
        assert!(s.participants.contains_key("A"));
        assert!(s.participants.contains_key("B"));
    }

    #[test]
    fn sequence_self_message() {
        let src = "sequenceDiagram\n  participant A\n  A->>A: 考える\n";
        let d = parse(src).expect("should parse");
        let Diagram::Sequence(s) = d else { panic!() };
        assert_eq!(s.items.len(), 1);
        let SequenceItem::Message(ref m) = s.items[0] else {
            panic!()
        };
        assert_eq!(m.from, "A");
        assert_eq!(m.to, "A");
    }

    #[test]
    fn sequence_open_arrow_maps_to_triangle() {
        // `->` maps to Solid + Triangle (compat: open arrow not rendered as open).
        let src = "sequenceDiagram\n  A->B: msg\n";
        let d = parse(src).expect("should parse");
        let Diagram::Sequence(s) = d else { panic!() };
        let SequenceItem::Message(ref m) = s.items[0] else {
            panic!()
        };
        assert_eq!(m.line, LineStyle::Solid);
        assert_eq!(m.arrow, ArrowType::Triangle);
    }

    #[test]
    fn sequence_dashed_open_arrow_maps_to_dashed_triangle() {
        let src = "sequenceDiagram\n  A-->B: msg\n";
        let d = parse(src).expect("should parse");
        let Diagram::Sequence(s) = d else { panic!() };
        let SequenceItem::Message(ref m) = s.items[0] else {
            panic!()
        };
        assert_eq!(m.line, LineStyle::Dashed);
        assert_eq!(m.arrow, ArrowType::Triangle);
    }

    #[test]
    fn note_is_unsupported() {
        let src = "sequenceDiagram\n  participant A\n  Note over A: text\n  A->>A: ok\n";
        let errs = parse(src).expect_err("Note should be unsupported");
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unsupported")
                    && e.message.to_lowercase().contains("note")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn loop_is_unsupported() {
        let src = "sequenceDiagram\n  participant A\n  loop every second\n    A->>A: tick\n  end\n";
        let errs = parse(src).expect_err("loop should be unsupported");
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unsupported") && e.message.contains("loop")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn semicolon_in_sequence_is_unsupported() {
        let src = "sequenceDiagram\n  A->>B: hello; B->>A: world\n";
        let errs = parse(src).expect_err("semicolon should be unsupported");
        assert!(
            errs.iter().any(|e| e.message.contains("semicolon")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn multiple_errors_collected() {
        // Both RL direction error and subgraph error should be reported together.
        let src = "flowchart RL\n  subgraph foo\n    A --> B\n  end\n";
        let errs = parse(src).expect_err("should have errors");
        assert!(errs.len() >= 2, "expected multiple errors, got: {errs:?}");
    }

    // -----------------------------------------------------------------------
    // Fix 1: Chain notation tests
    // -----------------------------------------------------------------------

    #[test]
    fn chain_three_nodes() {
        let src = "flowchart TD\n  A --> B --> C\n";
        let d = parse(src).expect("chain should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.nodes.len(), 3, "expected 3 nodes");
        assert_eq!(g.edges.len(), 2, "expected 2 edges from chain A-->B-->C");
        assert_eq!(g.edges[0].from, "A");
        assert_eq!(g.edges[0].to, "B");
        assert_eq!(g.edges[1].from, "B");
        assert_eq!(g.edges[1].to, "C");
    }

    #[test]
    fn chain_four_nodes() {
        let src = "flowchart TD\n  A --> B --> C --> D\n";
        let d = parse(src).expect("four-node chain should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.edges.len(), 3);
        assert_eq!(g.edges[0].from, "A");
        assert_eq!(g.edges[2].to, "D");
    }

    #[test]
    fn chain_with_pipe_label() {
        // Mixed chain: A -->|x| B --> C
        let src = "flowchart TD\n  A -->|x| B --> C\n";
        let d = parse(src).expect("mixed chain should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.edges.len(), 2);
        assert_eq!(g.edges[0].label.as_deref(), Some("x"));
        assert_eq!(g.edges[1].label, None);
        assert_eq!(g.edges[1].from, "B");
        assert_eq!(g.edges[1].to, "C");
    }

    // -----------------------------------------------------------------------
    // Fix 2: Pipe label with `|` in label content
    // -----------------------------------------------------------------------

    #[test]
    fn pipe_label_with_pipe_in_content_is_error() {
        // `A -->|a|b| B` — the label closes at the first `|`, leaving `b` as
        // surplus tokens before ` B`, which should error.
        let src = "flowchart TD\n  A -->|a|b| B\n";
        // After fix 2, `b|` is not a valid node so parse_node_with_label returns None,
        // meaning try_parse_edge_chain returns None (not an edge), falling through to
        // "unrecognised statement" — still an error, just a different message.
        // The important invariant: it must NOT silently produce label="a", to="b".
        let result = parse(src);
        match result {
            Err(_) => {} // any error is correct
            Ok(d) => {
                let Diagram::Graph(g) = d else { panic!() };
                // If it somehow parsed, assert it didn't incorrectly set to="b".
                for e in &g.edges {
                    assert_ne!(
                        e.to, "b",
                        "pipe label bug: to-node was incorrectly set to `b`"
                    );
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Fix 3: Stadium notation tests
    // -----------------------------------------------------------------------

    #[test]
    fn stadium_node_is_unsupported() {
        let src = "flowchart TD\n  A([丸い])\n";
        let errs = parse(src).expect_err("stadium shape should be unsupported");
        assert!(
            errs.iter().any(|e| e.message.contains("unsupported")
                && (e.message.contains("stadium") || e.message.contains("circle"))),
            "got: {errs:?}"
        );
    }

    #[test]
    fn circle_node_is_unsupported() {
        let src = "flowchart TD\n  A((丸))\n";
        let errs = parse(src).expect_err("circle shape should be unsupported");
        assert!(
            errs.iter().any(|e| e.message.contains("unsupported")
                && (e.message.contains("stadium") || e.message.contains("circle"))),
            "got: {errs:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Fix 5: Multi-target `&` tests
    // -----------------------------------------------------------------------

    #[test]
    fn multi_target_ampersand_is_unsupported() {
        let src = "flowchart TD\n  A --> B & C\n";
        let errs = parse(src).expect_err("multi-target & should be unsupported");
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unsupported") && e.message.contains("&")),
            "got: {errs:?}"
        );
    }
}
