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
    ArrowType, ClassDiagram, ClassNode, ClassRelation, Diagram, Direction, Edge, EndMarker,
    Endpoint, ErAttribute, ErDiagram, ErEntity, ErRelation, GraphDiagram, LineStyle, Message, Node,
    Participant, SequenceDiagram, SequenceItem, State, StateDiagram, Transition,
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
            "empty diagram: expected `flowchart`, `sequenceDiagram`, or `stateDiagram-v2` header",
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
    } else if let Some(rest) = strip_keyword_ci(header_trimmed, "stateDiagram-v2")
        .or_else(|| strip_keyword_ci(header_trimmed, "stateDiagram"))
    {
        if !rest.trim().is_empty() {
            errors.push(Diagnostic::new(
                "unexpected tokens after `stateDiagram`; the header must be on its own line",
                header_offset..header_offset + header_line.len(),
            ));
        }
        parse_state(&lines[1..], source, &mut errors)
    } else if let Some(rest) = strip_keyword_ci(header_trimmed, "classDiagram") {
        if !rest.trim().is_empty() {
            errors.push(Diagnostic::new(
                "unexpected tokens after `classDiagram`; the header must be on its own line",
                header_offset..header_offset + header_line.len(),
            ));
        }
        parse_class(&lines[1..], source, &mut errors)
    } else if let Some(rest) = strip_keyword_ci(header_trimmed, "erDiagram") {
        if !rest.trim().is_empty() {
            errors.push(Diagnostic::new(
                "unexpected tokens after `erDiagram`; the header must be on its own line",
                header_offset..header_offset + header_line.len(),
            ));
        }
        parse_er(&lines[1..], source, &mut errors)
    } else {
        errors.push(Diagnostic::new(
            format!(
                "unrecognised diagram header `{}`; expected `flowchart`, `graph`, `sequenceDiagram`, `stateDiagram-v2`, `classDiagram`, or `erDiagram`",
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
            // `participant X as Label` or `participant X`. The needle `" as "`
            // carries its own surrounding spaces, which is what enforces the
            // word boundary — a bare `as` inside an id would not match.
            let (id, label) = if let Some(idx) = rest.find(" as ") {
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
// State diagram parser
// ---------------------------------------------------------------------------

/// Parse a Mermaid `stateDiagram-v2` (or `stateDiagram`) body.
///
/// Supported:
/// - `[*] --> S` (initial), `S --> [*]` (final)
/// - `S --> T` and `S --> T : label`
/// - `state "long description" as s` and `state s`
/// - auto-declaration of states referenced only in transitions
///
/// Unsupported constructs (composite `state s { … }`, `direction`, `note`,
/// fork/join/choice `<<…>>`, concurrency `--`, `state s : description`) are
/// reported as positioned "unsupported" errors rather than silently dropped.
fn parse_state(
    lines: &[(usize, &str)],
    _source: &str,
    errors: &mut Vec<Diagnostic>,
) -> Result<Diagram, Vec<Diagnostic>> {
    // Explicit state declarations, in source order: id -> (label, decl span).
    let mut decls: IndexMap<String, (String, Range<usize>)> = IndexMap::new();
    // Raw transitions collected in source order.
    struct RawTrans {
        from: Endpoint,
        to: Endpoint,
        label: Option<String>,
    }
    let mut transitions: Vec<RawTrans> = Vec::new();

    for &(offset, line) in lines {
        let trimmed = line.trim();
        let span = offset..offset + line.len();

        // Semicolon separator (we only handle newline-separated statements).
        if trimmed.contains(';') {
            errors.push(Diagnostic::new(
                "unsupported: semicolon statement separator; use newlines instead (kozue does not support this yet)",
                span,
            ));
            continue;
        }

        // Composite states: any brace opens/closes a nested region.
        if trimmed.contains('{') || trimmed == "}" {
            errors.push(Diagnostic::new(
                "unsupported: composite/nested state (`state s { … }`); kozue does not support this yet",
                span,
            ));
            continue;
        }

        // Fork / join / choice / history pseudostates use `<<…>>` stereotypes.
        if trimmed.contains("<<") {
            errors.push(Diagnostic::new(
                "unsupported: fork/join/choice/history pseudostate (`<<…>>`); kozue does not support this yet",
                span,
            ));
            continue;
        }

        // Transitions (contain the `-->` arrow). Checked before the `direction`,
        // `note`, and `state` keyword guards so that a state whose id happens to
        // be one of those keywords (`note --> A`, `direction --> A`) is still
        // parsed as a transition rather than misreported as an unsupported feature.
        if trimmed.contains("-->") {
            match parse_state_transition(trimmed) {
                Ok((from, to, label)) => {
                    if matches!(from, Endpoint::Initial) && matches!(to, Endpoint::Final) {
                        errors.push(Diagnostic::new(
                            "`[*] --> [*]` is not valid; initial pseudostate cannot transition directly to final pseudostate",
                            span,
                        ));
                        continue;
                    }
                    transitions.push(RawTrans { from, to, label });
                }
                Err(msg) => errors.push(Diagnostic::new(msg, span)),
            }
            continue;
        }

        // `direction TB/LR/…` — kozue state layout is fixed top-down.
        if strip_keyword_ci(trimmed, "direction").is_some() {
            errors.push(Diagnostic::new(
                "unsupported: direction in state diagrams; kozue lays state diagrams top-down (kozue does not support this yet)",
                span,
            ));
            continue;
        }

        // Notes.
        if strip_keyword_ci(trimmed, "note").is_some() {
            errors.push(Diagnostic::new(
                "unsupported: note (kozue does not support this yet)",
                span,
            ));
            continue;
        }

        // State declarations: `state "desc" as id` or `state id`.
        if let Some(rest) = strip_keyword_ci(trimmed, "state") {
            match parse_state_decl(rest.trim()) {
                Ok((id, label)) => {
                    if decls.contains_key(&id) {
                        errors.push(Diagnostic::new(
                            format!("duplicate state declaration `{id}`"),
                            span,
                        ));
                    } else {
                        decls.insert(id, (label, span));
                    }
                }
                Err(msg) => errors.push(Diagnostic::new(msg, span)),
            }
            continue;
        }

        // Unrecognised (e.g. `S : description` state-body text, bare ids).
        errors.push(Diagnostic::new(
            format!(
                "syntax error: unrecognised statement `{}`",
                trimmed.chars().take(40).collect::<String>()
            ),
            span,
        ));
    }

    // Build the diagram: explicit declarations first (source order), then
    // auto-declare any state referenced only in transitions.
    let mut diagram = StateDiagram::new();
    for (id, (label, _span)) in &decls {
        diagram
            .states
            .insert(id.clone(), State::new(id.clone(), label.clone()));
    }
    for rt in &transitions {
        for ep in [&rt.from, &rt.to] {
            if let Endpoint::State(id) = ep {
                if !diagram.states.contains_key(id) {
                    diagram
                        .states
                        .insert(id.clone(), State::new(id.clone(), id.clone()));
                }
            }
        }
    }
    for rt in transitions {
        diagram
            .transitions
            .push(Transition::new(rt.from, rt.to, rt.label));
    }

    if errors.is_empty() {
        Ok(Diagram::State(diagram))
    } else {
        Err(errors.clone())
    }
}

/// Parse a state transition line `FROM --> TO` or `FROM --> TO : label`.
///
/// Returns `(from_endpoint, to_endpoint, optional_label)`.
fn parse_state_transition(trimmed: &str) -> Result<(Endpoint, Endpoint, Option<String>), String> {
    let idx = trimmed
        .find("-->")
        .expect("caller guarantees the line contains `-->`");
    let from_part = trimmed[..idx].trim();
    let after = trimmed[idx + 3..].trim();

    // Split the target from an optional `: label`.
    let (to_part, label) = match after.find(':') {
        Some(ci) => {
            let target = after[..ci].trim();
            let lbl = after[ci + 1..].trim();
            let label = if lbl.is_empty() {
                None
            } else {
                Some(lbl.to_string())
            };
            (target, label)
        }
        None => (after, None),
    };

    let from = parse_state_endpoint(from_part, true)?;
    let to = parse_state_endpoint(to_part, false)?;
    Ok((from, to, label))
}

/// Parse a single transition endpoint. `[*]` maps to [`Endpoint::Initial`] on the
/// left (`is_source`) and [`Endpoint::Final`] on the right; otherwise the token
/// must be a bare state identifier.
fn parse_state_endpoint(part: &str, is_source: bool) -> Result<Endpoint, String> {
    let part = part.trim();
    if part == "[*]" {
        return Ok(if is_source {
            Endpoint::Initial
        } else {
            Endpoint::Final
        });
    }
    match split_id(part) {
        Some((id, rest)) if rest.trim().is_empty() => Ok(Endpoint::State(id)),
        _ => Err(format!(
            "syntax error: expected a state identifier or `[*]`, got `{}`",
            part.chars().take(40).collect::<String>()
        )),
    }
}

/// Parse the text following the `state` keyword: `"long description" as id` or a
/// bare `id`.
fn parse_state_decl(rest: &str) -> Result<(String, String), String> {
    if rest.is_empty() {
        return Err("expected a state identifier after `state`".to_string());
    }
    // Quoted display form: `state "desc" as id`.
    if let Some(after_open) = rest.strip_prefix('"') {
        let close = after_open
            .find('"')
            .ok_or("unterminated quoted state description")?;
        let label = after_open[..close].to_string();
        let after = after_open[close + 1..].trim();
        let id = after
            .strip_prefix("as ")
            .or_else(|| after.strip_prefix("AS "))
            .ok_or("expected `as <id>` after quoted state description")?
            .trim();
        match split_id(id) {
            Some((id, tail)) if tail.trim().is_empty() => Ok((id, label)),
            _ => Err(format!(
                "invalid state identifier `{}`",
                id.chars().take(40).collect::<String>()
            )),
        }
    } else {
        // Bare `state id`. Reject trailing tokens (e.g. `state s : desc`,
        // which is a state-description assignment kozue does not support).
        match split_id(rest) {
            Some((id, tail)) if tail.trim().is_empty() => Ok((id.clone(), id)),
            Some((_, tail)) if tail.trim_start().starts_with(':') => Err(
                "unsupported: state description (`state s : …`); kozue does not support this yet"
                    .to_string(),
            ),
            _ => Err(format!(
                "invalid state declaration `{}`",
                rest.chars().take(40).collect::<String>()
            )),
        }
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
///
/// Uses `str::get` for the prefix slice so a non-ASCII first character (whose
/// byte length straddles `keyword.len()`) returns `None` rather than panicking.
fn strip_keyword_ci<'a>(s: &'a str, keyword: &str) -> Option<&'a str> {
    match s.get(..keyword.len()) {
        Some(prefix)
            if prefix.eq_ignore_ascii_case(keyword)
                // Must be followed by whitespace or end-of-string (word boundary).
                && (s.len() == keyword.len()
                    || s[keyword.len()..]
                        .starts_with(|c: char| c.is_ascii_whitespace() || c == '\0')) =>
        {
            Some(&s[keyword.len()..])
        }
        _ => None,
    }
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

// ---------------------------------------------------------------------------
// Class diagram parser
// ---------------------------------------------------------------------------

/// Mermaid `classDiagram` relation connectors: `(token, from_marker, to_marker, line)`.
/// `from_marker`/`to_marker` are the markers drawn at the left/right end of the
/// token as written, matching Mermaid's own left-to-right convention. Mermaid
/// class diagrams allow a symbol at either end, so both spelling directions of
/// each relation are listed (e.g. `A <|-- B` puts the hollow triangle at A's
/// end; `A --|> B` puts it at B's end).
const CLASS_CONNECTORS: &[(&str, EndMarker, EndMarker, LineStyle)] = &[
    // Generalization / realization (hollow triangle).
    (
        "<|--",
        EndMarker::HollowTriangle,
        EndMarker::None,
        LineStyle::Solid,
    ),
    (
        "--|>",
        EndMarker::None,
        EndMarker::HollowTriangle,
        LineStyle::Solid,
    ),
    (
        "<|..",
        EndMarker::HollowTriangle,
        EndMarker::None,
        LineStyle::Dashed,
    ),
    (
        "..|>",
        EndMarker::None,
        EndMarker::HollowTriangle,
        LineStyle::Dashed,
    ),
    // Composition (filled diamond).
    (
        "*--",
        EndMarker::FilledDiamond,
        EndMarker::None,
        LineStyle::Solid,
    ),
    (
        "--*",
        EndMarker::None,
        EndMarker::FilledDiamond,
        LineStyle::Solid,
    ),
    // Aggregation (hollow diamond).
    (
        "o--",
        EndMarker::HollowDiamond,
        EndMarker::None,
        LineStyle::Solid,
    ),
    (
        "--o",
        EndMarker::None,
        EndMarker::HollowDiamond,
        LineStyle::Solid,
    ),
    // Association / dependency (open arrow).
    (
        "-->",
        EndMarker::None,
        EndMarker::OpenArrow,
        LineStyle::Solid,
    ),
    (
        "<--",
        EndMarker::OpenArrow,
        EndMarker::None,
        LineStyle::Solid,
    ),
    (
        "..>",
        EndMarker::None,
        EndMarker::OpenArrow,
        LineStyle::Dashed,
    ),
    (
        "<..",
        EndMarker::OpenArrow,
        EndMarker::None,
        LineStyle::Dashed,
    ),
    // Plain association (no markers).
    ("--", EndMarker::None, EndMarker::None, LineStyle::Solid),
    ("..", EndMarker::None, EndMarker::None, LineStyle::Dashed),
];

/// Split a string into whitespace-separated tokens, treating a `"..."` run as
/// a single token (so quoted multiplicities/comments containing spaces are
/// not split apart).
fn tokenize_ws_quoted(s: &str) -> Vec<String> {
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

/// Split a relation-side token group into (identifier, optional multiplicity).
/// The multiplicity may appear before or after the identifier.
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
    Ok((id, mult))
}

/// Parsed class relation, before validating referenced classes exist.
struct ParsedClassRelation {
    from: String,
    to: String,
    from_marker: EndMarker,
    to_marker: EndMarker,
    line: LineStyle,
    label: Option<String>,
    from_mult: Option<String>,
    to_mult: Option<String>,
}

/// Try to parse a Mermaid classDiagram relation line, e.g.
/// `A <|-- B`, `A "1" --> "*" B : label`. Returns `None` if the line does not
/// contain a recognised relation connector token.
fn try_parse_class_relation(trimmed: &str) -> Option<Result<ParsedClassRelation, String>> {
    let (rel_part, label) = match trimmed.find(':') {
        Some(idx) => (trimmed[..idx].trim(), {
            let l = trimmed[idx + 1..].trim();
            if l.is_empty() {
                None
            } else {
                Some(l.to_string())
            }
        }),
        None => (trimmed, None),
    };
    let tokens = tokenize_ws_quoted(rel_part);
    let conn_idx = tokens
        .iter()
        .position(|t| CLASS_CONNECTORS.iter().any(|(tok, ..)| tok == t))?;
    let (_, from_marker, to_marker, line) = *CLASS_CONNECTORS
        .iter()
        .find(|(tok, ..)| tok == &tokens[conn_idx])
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
    Some(Ok(ParsedClassRelation {
        from,
        to,
        from_marker,
        to_marker,
        line,
        label,
        from_mult,
        to_mult,
    }))
}

/// Parse a single class member line (inside a `class Foo { ... }` block) into
/// its pre-formatted display string. Returns `Some("stereotype", name)` for
/// `<<stereotype>>` lines via the caller checking the prefix separately.
fn parse_class_member(trimmed: &str, attributes: &mut Vec<String>, methods: &mut Vec<String>) {
    let vis = trimmed.chars().next().filter(|c| "+-#~".contains(*c));
    let rest = if let Some(v) = vis {
        trimmed[v.len_utf8()..].trim_start()
    } else {
        trimmed
    };
    let vis_str = vis.map(|c| c.to_string()).unwrap_or_default();

    if let Some(paren_idx) = rest.find('(') {
        let name = rest[..paren_idx].trim();
        let close = rest[paren_idx..]
            .find(')')
            .map(|o| paren_idx + o)
            .unwrap_or(rest.len());
        let args = rest.get(paren_idx + 1..close).unwrap_or("");
        let after = rest.get(close + 1..).unwrap_or("").trim();
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
        attributes.push(format!("{vis_str}{}", rest.trim()));
    }
}

/// Handle one member line inside a `class Foo { ... }` body (or a colon-omitted
/// `Foo : member` statement). Dispatches `<<stereotype>>` annotations, rejects
/// generics, and otherwise routes to [`parse_class_member`]. `span` is used for
/// any diagnostics.
fn handle_class_member_line(
    member: &str,
    class_id: &str,
    diagram: &mut ClassDiagram,
    span: Range<usize>,
    errors: &mut Vec<Diagnostic>,
) {
    if let Some(rest) = member.strip_prefix("<<") {
        if let Some(close) = rest.find(">>") {
            let stereotype = rest[..close].trim().to_string();
            diagram.classes[class_id].stereotype = Some(stereotype);
        } else {
            errors.push(Diagnostic::new(
                "unterminated `<<stereotype>>` annotation (missing `>>`)",
                span,
            ));
        }
        return;
    }
    if member.contains('~') {
        errors.push(Diagnostic::new(
            "unsupported: generic type parameters (`~T~`); kozue does not support this yet",
            span,
        ));
        return;
    }
    let node = &mut diagram.classes[class_id];
    let mut attrs = std::mem::take(&mut node.attributes);
    let mut methods = std::mem::take(&mut node.methods);
    parse_class_member(member, &mut attrs, &mut methods);
    let node = &mut diagram.classes[class_id];
    node.attributes = attrs;
    node.methods = methods;
}

fn parse_class(
    lines: &[(usize, &str)],
    _source: &str,
    errors: &mut Vec<Diagnostic>,
) -> Result<Diagram, Vec<Diagnostic>> {
    let mut diagram = ClassDiagram::new(Direction::Down);
    let mut relations: Vec<(ParsedClassRelation, Range<usize>)> = Vec::new();

    let ensure_class = |diagram: &mut ClassDiagram, id: &str| {
        if !diagram.classes.contains_key(id) {
            diagram.classes.insert(
                id.to_string(),
                ClassNode::new(id.to_string(), id.to_string()),
            );
        }
    };

    let mut i = 0usize;
    while i < lines.len() {
        let (offset, line) = lines[i];
        let trimmed = line.trim();
        let span = offset..offset + line.len();

        if let Some(rest) = strip_keyword_ci(trimmed, "direction") {
            match rest.trim().to_ascii_uppercase().as_str() {
                "LR" => diagram.direction = Direction::Right,
                "TD" | "TB" => diagram.direction = Direction::Down,
                other => errors.push(Diagnostic::new(
                    format!("unknown classDiagram direction `{other}`; expected TD, TB, or LR"),
                    span,
                )),
            }
            i += 1;
            continue;
        }

        let unsupported_kw: &[&str] = &["namespace", "note", "click", "style", "cssClass"];
        if let Some(kw) = unsupported_kw
            .iter()
            .find(|kw| strip_keyword_ci(trimmed, kw).is_some())
        {
            errors.push(Diagnostic::new(
                format!("unsupported: {kw} (kozue does not support this yet)"),
                span,
            ));
            // These constructs may open a `{ ... }` block; skip forward past a
            // matching closing brace line so we don't misparse its contents.
            if trimmed.ends_with('{') {
                i += 1;
                while i < lines.len() && lines[i].1.trim() != "}" {
                    i += 1;
                }
            }
            i += 1;
            continue;
        }

        // Standalone stereotype annotation: `<<interface>> Foo`.
        if let Some(rest) = trimmed.strip_prefix("<<") {
            if let Some(close) = rest.find(">>") {
                let stereotype = rest[..close].trim().to_string();
                let name = rest[close + 2..].trim();
                if name.is_empty() {
                    errors.push(Diagnostic::new(
                        "expected a class name after `<<stereotype>>`",
                        span,
                    ));
                } else if let Some((id, tail)) = split_id(name) {
                    if !tail.trim().is_empty() {
                        errors.push(Diagnostic::new(
                            format!(
                                "syntax error: unexpected tokens after class name `{}`",
                                name.chars().take(40).collect::<String>()
                            ),
                            span,
                        ));
                    } else {
                        ensure_class(&mut diagram, &id);
                        diagram.classes[&id].stereotype = Some(stereotype);
                    }
                } else {
                    errors.push(Diagnostic::new(
                        format!(
                            "syntax error: expected a class identifier, got `{}`",
                            name.chars().take(40).collect::<String>()
                        ),
                        span,
                    ));
                }
            } else {
                errors.push(Diagnostic::new(
                    "unterminated `<<stereotype>>` annotation (missing `>>`)",
                    span,
                ));
            }
            i += 1;
            continue;
        }

        // Class declaration: `class Foo`, `class Foo { ... }` (multi-line), or
        // `class Foo { +a: int }` (single-line inline; members split by `;`).
        if let Some(rest) = strip_keyword_ci(trimmed, "class") {
            let rest = rest.trim();
            // Body-shape detection (mirrors the native DSL). `~` in the name
            // portion means generics; inside an inline body it is handled per
            // member by `handle_class_member_line`.
            enum Body<'a> {
                None(&'a str),
                Inline(&'a str, &'a str),
                Multiline(&'a str),
                Malformed,
            }
            let body = if let Some(open) = rest.find('{') {
                if let Some(inner) = rest[open + 1..].strip_suffix('}') {
                    Body::Inline(rest[..open].trim(), inner.trim())
                } else if rest.trim_end().ends_with('{') {
                    Body::Multiline(rest[..open].trim())
                } else {
                    Body::Malformed
                }
            } else {
                Body::None(rest)
            };
            let name = match body {
                Body::None(n) | Body::Inline(n, _) | Body::Multiline(n) => n,
                Body::Malformed => {
                    errors.push(Diagnostic::new(
                        format!(
                            "syntax error: malformed class body `{}`",
                            trimmed.chars().take(40).collect::<String>()
                        ),
                        span,
                    ));
                    i += 1;
                    continue;
                }
            };
            if name.contains('~') {
                errors.push(Diagnostic::new(
                    "unsupported: generic type parameters (`~T~`); kozue does not support this yet",
                    span,
                ));
                i += 1;
                continue;
            }
            let Some((id, tail)) = split_id(name) else {
                errors.push(Diagnostic::new(
                    format!(
                        "syntax error: expected a class identifier, got `{}`",
                        name.chars().take(40).collect::<String>()
                    ),
                    span,
                ));
                i += 1;
                continue;
            };
            if !tail.trim().is_empty() {
                errors.push(Diagnostic::new(
                    format!(
                        "syntax error: unexpected tokens after class name `{}`",
                        name.chars().take(40).collect::<String>()
                    ),
                    span,
                ));
                i += 1;
                continue;
            }
            ensure_class(&mut diagram, &id);
            match body {
                Body::None(_) => {
                    i += 1;
                }
                Body::Inline(_, inner) => {
                    for member in inner.split(';') {
                        let member = member.trim();
                        if member.is_empty() {
                            continue;
                        }
                        handle_class_member_line(member, &id, &mut diagram, span.clone(), errors);
                    }
                    i += 1;
                }
                Body::Multiline(_) => {
                    i += 1;
                    loop {
                        if i >= lines.len() {
                            errors.push(Diagnostic::new(
                                format!("unterminated `class {id} {{ ... }}` block (missing `}}`)"),
                                span.clone(),
                            ));
                            break;
                        }
                        let (moff, mline) = lines[i];
                        let mtrim = mline.trim();
                        if mtrim == "}" {
                            i += 1;
                            break;
                        }
                        handle_class_member_line(
                            mtrim,
                            &id,
                            &mut diagram,
                            moff..moff + mline.len(),
                            errors,
                        );
                        i += 1;
                    }
                }
                Body::Malformed => unreachable!(),
            }
            continue;
        }

        // Relation line.
        if let Some(result) = try_parse_class_relation(trimmed) {
            match result {
                Ok(rel) => relations.push((rel, span)),
                Err(msg) => errors.push(Diagnostic::new(msg, span)),
            }
            i += 1;
            continue;
        }

        // Colon-omitted member statement: `Foo : +member` / `Foo : +move() void`.
        // The class is created implicitly if not already declared. Checked after
        // the relation attempt so labelled relations (`A --> B : label`) win.
        if let Some(ci) = trimmed.find(':') {
            let id_part = trimmed[..ci].trim();
            let member = trimmed[ci + 1..].trim();
            if let Some((id, tail)) = split_id(id_part) {
                if tail.trim().is_empty() && !member.is_empty() {
                    ensure_class(&mut diagram, &id);
                    handle_class_member_line(member, &id, &mut diagram, span, errors);
                    i += 1;
                    continue;
                }
            }
        }

        if trimmed.is_empty() {
            i += 1;
            continue;
        }

        errors.push(Diagnostic::new(
            format!(
                "syntax error: unrecognised statement `{}`",
                trimmed.chars().take(40).collect::<String>()
            ),
            span,
        ));
        i += 1;
    }

    for (rel, span) in relations {
        if rel.from == rel.to {
            errors.push(Diagnostic::new(
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
            rel.line,
            rel.label,
            rel.from_mult,
            rel.to_mult,
        ));
    }

    if errors.is_empty() {
        Ok(Diagram::Class(diagram))
    } else {
        Err(errors.clone())
    }
}

// ---------------------------------------------------------------------------
// ER diagram parser
// ---------------------------------------------------------------------------

/// Decode a Mermaid ER crow's-foot connector token, e.g. `||--o{`, into
/// `(from_marker, to_marker, line_style)`.
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

struct ParsedErRelation {
    from: String,
    to: String,
    from_marker: EndMarker,
    to_marker: EndMarker,
    line: LineStyle,
    label: Option<String>,
}

/// Try to parse an ER relation line, e.g. `CUSTOMER ||--o{ ORDER : places`.
/// Returns `None` if the line has no crow's-foot connector token.
fn try_parse_er_relation(trimmed: &str) -> Option<Result<ParsedErRelation, String>> {
    let (rel_part, label) = match trimmed.find(':') {
        Some(idx) => (trimmed[..idx].trim(), {
            let l = trimmed[idx + 1..].trim();
            if l.is_empty() {
                None
            } else {
                Some(l.to_string())
            }
        }),
        None => (trimmed, None),
    };
    let tokens: Vec<&str> = rel_part.split_whitespace().collect();
    if tokens.len() != 3 {
        return None;
    }
    let (from_marker, to_marker, line) = parse_crowfoot_token(tokens[1])?;
    Some(Ok(ParsedErRelation {
        from: tokens[0].to_string(),
        to: tokens[2].to_string(),
        from_marker,
        to_marker,
        line,
        label,
    }))
}

/// Parse one ER entity attribute line: `type name [PK|FK|UK]... ["comment"]`.
fn parse_er_attr_line(trimmed: &str) -> Result<ErAttribute, String> {
    let tokens = tokenize_ws_quoted(trimmed);
    if tokens.len() < 2 {
        return Err(format!(
            "syntax error: expected `type name` in entity attribute, got `{}`",
            trimmed.chars().take(40).collect::<String>()
        ));
    }
    let type_name = tokens[0].clone();
    let name = tokens[1].clone();
    let mut keys = Vec::new();
    let mut comment = None;
    for t in &tokens[2..] {
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

fn parse_er(
    lines: &[(usize, &str)],
    _source: &str,
    errors: &mut Vec<Diagnostic>,
) -> Result<Diagram, Vec<Diagnostic>> {
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
        let trimmed = line.trim();
        let span = offset..offset + line.len();

        if trimmed.is_empty() {
            i += 1;
            continue;
        }

        // Entity block: `NAME { ... }` (multi-line) or `NAME { a; b }`
        // (single-line inline; attributes split by `;`). Only enter this branch
        // when the text before `{` is a bare identifier, so a relation line
        // whose crow's-foot token contains `{` (e.g. `A ||--o{ B`) is not
        // misread as an entity block and instead falls through to the relation
        // parser below.
        if let Some((open, id)) = trimmed.find('{').and_then(|open| {
            let name = trimmed[..open].trim();
            match split_id(name) {
                Some((id, tail)) if tail.trim().is_empty() => Some((open, id)),
                _ => None,
            }
        }) {
            let inline_body = trimmed[open + 1..]
                .strip_suffix('}')
                .map(|inner| inner.trim());
            let is_multiline = inline_body.is_none() && trimmed.trim_end().ends_with('{');
            if inline_body.is_none() && !is_multiline {
                errors.push(Diagnostic::new(
                    format!(
                        "syntax error: malformed entity body `{}`",
                        trimmed.chars().take(40).collect::<String>()
                    ),
                    span,
                ));
                i += 1;
                continue;
            }
            ensure_entity(&mut diagram, &id);
            if let Some(inner) = inline_body {
                for attr in inner.split(';') {
                    let attr = attr.trim();
                    if attr.is_empty() {
                        continue;
                    }
                    match parse_er_attr_line(attr) {
                        Ok(a) => diagram.entities[&id].attributes.push(a),
                        Err(msg) => errors.push(Diagnostic::new(msg, span.clone())),
                    }
                }
                i += 1;
                continue;
            }
            i += 1;
            loop {
                if i >= lines.len() {
                    errors.push(Diagnostic::new(
                        format!("unterminated `{id} {{ ... }}` entity block (missing `}}`)"),
                        span.clone(),
                    ));
                    break;
                }
                let (moff, mline) = lines[i];
                let mtrim = mline.trim();
                if mtrim == "}" {
                    i += 1;
                    break;
                }
                match parse_er_attr_line(mtrim) {
                    Ok(attr) => diagram.entities[&id].attributes.push(attr),
                    Err(msg) => {
                        errors.push(Diagnostic::new(msg, moff..moff + mline.len()));
                    }
                }
                i += 1;
            }
            continue;
        }

        // Relation line.
        if let Some(result) = try_parse_er_relation(trimmed) {
            match result {
                Ok(rel) => {
                    if rel.from == rel.to {
                        errors.push(Diagnostic::new(
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
                Err(msg) => errors.push(Diagnostic::new(msg, span)),
            }
            i += 1;
            continue;
        }

        errors.push(Diagnostic::new(
            format!(
                "syntax error: unrecognised statement `{}`",
                trimmed.chars().take(40).collect::<String>()
            ),
            span,
        ));
        i += 1;
    }

    if errors.is_empty() {
        Ok(Diagram::Er(diagram))
    } else {
        Err(errors.clone())
    }
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

    // -----------------------------------------------------------------------
    // State diagram tests
    // -----------------------------------------------------------------------

    fn state(d: &Diagram) -> &StateDiagram {
        match d {
            Diagram::State(s) => s,
            other => panic!("expected state diagram, got {other:?}"),
        }
    }

    #[test]
    fn state_basic_v2() {
        let src =
            "stateDiagram-v2\n  [*] --> Idle\n  Idle --> Running : start\n  Running --> [*]\n";
        let d = parse(src).expect("should parse");
        let s = state(&d);
        assert!(s.states.contains_key("Idle"));
        assert!(s.states.contains_key("Running"));
        assert_eq!(s.transitions.len(), 3);
        assert_eq!(s.transitions[0].from, Endpoint::Initial);
        assert_eq!(s.transitions[0].to, Endpoint::State("Idle".into()));
        assert_eq!(s.transitions[1].label.as_deref(), Some("start"));
        assert_eq!(s.transitions[2].to, Endpoint::Final);
    }

    #[test]
    fn state_plain_header_also_accepted() {
        let src = "stateDiagram\n  [*] --> A\n  A --> [*]\n";
        let d = parse(src).expect("plain stateDiagram header should parse");
        assert_eq!(state(&d).transitions.len(), 2);
    }

    #[test]
    fn state_quoted_description_and_alias() {
        let src = "stateDiagram-v2\n  state \"Long Name\" as s1\n  [*] --> s1\n";
        let d = parse(src).expect("should parse");
        let s = state(&d);
        assert_eq!(s.states.get("s1").unwrap().label, "Long Name");
    }

    #[test]
    fn state_bare_declaration() {
        let src = "stateDiagram-v2\n  state Alone\n  [*] --> Alone\n";
        let d = parse(src).expect("should parse");
        let s = state(&d);
        assert_eq!(s.states.get("Alone").unwrap().label, "Alone");
    }

    #[test]
    fn state_transition_without_label() {
        let src = "stateDiagram-v2\n  A --> B\n";
        let d = parse(src).expect("should parse");
        assert_eq!(state(&d).transitions[0].label, None);
    }

    #[test]
    fn state_initial_to_final_is_error() {
        let src = "stateDiagram-v2\n  [*] --> [*]\n";
        let errs = parse(src).expect_err("[*] --> [*] should be rejected");
        assert!(
            errs.iter().any(|e| e.message.contains("[*] --> [*]")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn state_composite_is_unsupported() {
        let src = "stateDiagram-v2\n  state Outer {\n    [*] --> Inner\n  }\n";
        let errs = parse(src).expect_err("composite state should be unsupported");
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unsupported") && e.message.contains("composite")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn state_fork_join_is_unsupported() {
        let src = "stateDiagram-v2\n  state fork_state <<fork>>\n";
        let errs = parse(src).expect_err("fork should be unsupported");
        assert!(
            errs.iter().any(|e| e.message.contains("unsupported")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn state_direction_is_unsupported() {
        let src = "stateDiagram-v2\n  direction LR\n  [*] --> A\n";
        let errs = parse(src).expect_err("direction should be unsupported");
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unsupported") && e.message.contains("direction")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn state_note_is_unsupported() {
        let src = "stateDiagram-v2\n  [*] --> A\n  note right of A : hi\n";
        let errs = parse(src).expect_err("note should be unsupported");
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unsupported") && e.message.contains("note")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn state_description_assignment_is_unsupported() {
        let src = "stateDiagram-v2\n  state s\n  s : some description\n";
        let errs = parse(src).expect_err("state-description text should be rejected");
        assert!(!errs.is_empty(), "got: {errs:?}");
    }

    #[test]
    fn state_duplicate_declaration_is_error() {
        let src = "stateDiagram-v2\n  state \"A\" as x\n  state \"B\" as x\n";
        let errs = parse(src).expect_err("duplicate decl should error");
        assert!(
            errs.iter().any(|e| e.message.contains("duplicate")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn state_auto_declares_referenced_states() {
        let src = "stateDiagram-v2\n  A --> B\n  B --> C\n";
        let d = parse(src).expect("should parse");
        let s = state(&d);
        for id in ["A", "B", "C"] {
            assert!(s.states.contains_key(id), "missing {id}");
        }
    }

    #[test]
    fn state_id_colliding_with_keyword_is_a_transition() {
        // `note` / `direction` are legal Mermaid state ids; as a transition
        // source they must be parsed as states, not misreported as unsupported.
        let src = "stateDiagram-v2\n  note --> A\n  direction --> B\n";
        let d = parse(src).expect("keyword-named states should parse");
        let s = state(&d);
        assert!(s.states.contains_key("note"));
        assert!(s.states.contains_key("direction"));
        assert_eq!(s.transitions.len(), 2);
    }

    #[test]
    fn state_non_ascii_unrecognised_line_does_not_panic() {
        // A non-ASCII bare line must yield a diagnostic, not a byte-boundary panic.
        let src = "stateDiagram-v2\n  ああ\n";
        let errs = parse(src).expect_err("non-ascii bare line should error");
        assert!(!errs.is_empty());
    }

    // -----------------------------------------------------------------------
    // Class diagram tests
    // -----------------------------------------------------------------------

    fn class_diagram(d: &Diagram) -> &kozue_ir::ClassDiagram {
        match d {
            Diagram::Class(c) => c,
            other => panic!("expected class diagram, got {other:?}"),
        }
    }

    #[test]
    fn class_basic_block_and_relation() {
        let src = r#"classDiagram
  class Animal {
    +String name
    +makeSound() void
  }
  class Dog {
    +bark() void
  }
  Dog <|-- Animal
"#;
        let d = parse(src).expect("should parse");
        let c = class_diagram(&d);
        assert_eq!(c.classes.len(), 2);
        assert!(c.classes["Animal"].attributes[0].contains("name"));
        assert_eq!(c.classes["Animal"].methods[0], "+makeSound(): void");
        assert_eq!(c.relations.len(), 1);
        assert_eq!(c.relations[0].from, "Dog");
        assert_eq!(c.relations[0].to, "Animal");
        assert_eq!(c.relations[0].from_marker, EndMarker::HollowTriangle);
        assert_eq!(c.relations[0].to_marker, EndMarker::None);
    }

    #[test]
    fn class_mermaid_direction_inheritance() {
        let src = "classDiagram\n  Dog <|-- Animal\n";
        let d = parse(src).expect("should parse");
        let c = class_diagram(&d);
        assert_eq!(c.relations[0].from, "Dog");
        assert_eq!(c.relations[0].to, "Animal");
        assert_eq!(c.relations[0].from_marker, EndMarker::HollowTriangle);
        assert_eq!(c.relations[0].to_marker, EndMarker::None);
        assert_eq!(c.relations[0].line, LineStyle::Solid);
    }

    /// Helper: parse a single-relation classDiagram body and return the relation.
    fn class_one_relation(rel_line: &str) -> ClassRelation {
        let src = format!("classDiagram\n  {rel_line}\n");
        let d = parse(&src).unwrap_or_else(|e| panic!("`{rel_line}` should parse: {e:?}"));
        let c = class_diagram(&d);
        assert_eq!(c.relations.len(), 1, "`{rel_line}` produced != 1 relation");
        c.relations[0].clone()
    }

    #[test]
    fn class_all_connector_directions_are_accepted() {
        // Every token, forward and reverse, must parse and place the marker on
        // the end the glyph points at. (token, from_marker, to_marker, line)
        let cases: &[(&str, EndMarker, EndMarker, LineStyle)] = &[
            (
                "A <|-- B",
                EndMarker::HollowTriangle,
                EndMarker::None,
                LineStyle::Solid,
            ),
            (
                "A --|> B",
                EndMarker::None,
                EndMarker::HollowTriangle,
                LineStyle::Solid,
            ),
            (
                "A <|.. B",
                EndMarker::HollowTriangle,
                EndMarker::None,
                LineStyle::Dashed,
            ),
            (
                "A ..|> B",
                EndMarker::None,
                EndMarker::HollowTriangle,
                LineStyle::Dashed,
            ),
            (
                "A *-- B",
                EndMarker::FilledDiamond,
                EndMarker::None,
                LineStyle::Solid,
            ),
            (
                "A --* B",
                EndMarker::None,
                EndMarker::FilledDiamond,
                LineStyle::Solid,
            ),
            (
                "A o-- B",
                EndMarker::HollowDiamond,
                EndMarker::None,
                LineStyle::Solid,
            ),
            (
                "A --o B",
                EndMarker::None,
                EndMarker::HollowDiamond,
                LineStyle::Solid,
            ),
            (
                "A --> B",
                EndMarker::None,
                EndMarker::OpenArrow,
                LineStyle::Solid,
            ),
            (
                "A <-- B",
                EndMarker::OpenArrow,
                EndMarker::None,
                LineStyle::Solid,
            ),
            (
                "A ..> B",
                EndMarker::None,
                EndMarker::OpenArrow,
                LineStyle::Dashed,
            ),
            (
                "A <.. B",
                EndMarker::OpenArrow,
                EndMarker::None,
                LineStyle::Dashed,
            ),
            ("A -- B", EndMarker::None, EndMarker::None, LineStyle::Solid),
            (
                "A .. B",
                EndMarker::None,
                EndMarker::None,
                LineStyle::Dashed,
            ),
        ];
        for &(line, from_m, to_m, ls) in cases {
            let r = class_one_relation(line);
            assert_eq!(r.from, "A", "`{line}` from");
            assert_eq!(r.to, "B", "`{line}` to");
            assert_eq!(r.from_marker, from_m, "`{line}` from_marker");
            assert_eq!(r.to_marker, to_m, "`{line}` to_marker");
            assert_eq!(r.line, ls, "`{line}` line");
        }
    }

    #[test]
    fn class_forward_and_reverse_tokens_are_mirror_images() {
        // `A <|-- B` and `B --|> A` describe the same UML relation; swapping
        // from/to must yield swapped markers with the same line style.
        for (fwd, rev) in [
            ("A <|-- B", "B --|> A"),
            ("A *-- B", "B --* A"),
            ("A o-- B", "B --o A"),
            ("A --> B", "B <-- A"),
            ("A ..|> B", "B <|.. A"),
            ("A ..> B", "B <.. A"),
        ] {
            let f = class_one_relation(fwd);
            let r = class_one_relation(rev);
            assert_eq!(f.from, r.to, "{fwd} / {rev}: endpoints must swap");
            assert_eq!(f.to, r.from, "{fwd} / {rev}: endpoints must swap");
            assert_eq!(
                f.from_marker, r.to_marker,
                "{fwd} / {rev}: markers must mirror"
            );
            assert_eq!(
                f.to_marker, r.from_marker,
                "{fwd} / {rev}: markers must mirror"
            );
            assert_eq!(f.line, r.line, "{fwd} / {rev}: line style must match");
        }
    }

    #[test]
    fn class_stereotype_annotation() {
        let src = "classDiagram\n  <<interface>> Shape\n  class Shape\n";
        let d = parse(src).expect("should parse");
        let c = class_diagram(&d);
        assert_eq!(c.classes["Shape"].stereotype.as_deref(), Some("interface"));
    }

    #[test]
    fn class_multiplicity_and_label() {
        let src = "classDiagram\n  Customer \"1\" --> \"*\" Order : places\n";
        let d = parse(src).expect("should parse");
        let c = class_diagram(&d);
        let r = &c.relations[0];
        assert_eq!(r.from_mult.as_deref(), Some("1"));
        assert_eq!(r.to_mult.as_deref(), Some("*"));
        assert_eq!(r.label.as_deref(), Some("places"));
    }

    #[test]
    fn class_colon_omitted_member_notation() {
        // F2: `ClassName : member` form, implicit class creation, `()` decides
        // method vs attribute.
        let src = "classDiagram\n  Animal : +String name\n  Animal : +move() void\n";
        let d = parse(src).expect("should parse");
        let c = class_diagram(&d);
        assert!(c.classes.contains_key("Animal"), "class auto-created");
        assert_eq!(c.classes["Animal"].attributes, vec!["+String name"]);
        assert_eq!(c.classes["Animal"].methods, vec!["+move(): void"]);
    }

    #[test]
    fn class_colon_omitted_does_not_break_relation_label() {
        // A labelled relation must still be parsed as a relation, not a member.
        let src = "classDiagram\n  A --> B : uses\n";
        let d = parse(src).expect("should parse");
        let c = class_diagram(&d);
        assert_eq!(c.relations.len(), 1);
        assert_eq!(c.relations[0].label.as_deref(), Some("uses"));
        assert!(c.classes["A"].attributes.is_empty());
    }

    #[test]
    fn class_single_line_inline_block() {
        // F3: `{` and `}` on the same line, members split by `;`.
        let src = "classDiagram\n  class Point { +x: int; +y: int; +dist(): float }\n";
        let d = parse(src).expect("should parse");
        let c = class_diagram(&d);
        assert_eq!(c.classes["Point"].attributes, vec!["+x: int", "+y: int"]);
        assert_eq!(c.classes["Point"].methods, vec!["+dist(): float"]);
    }

    #[test]
    fn class_namespace_is_unsupported() {
        let src = "classDiagram\n  namespace Foo {\n    class A\n  }\n";
        let errs = parse(src).expect_err("namespace should be unsupported");
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unsupported") && e.message.contains("namespace")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn class_generics_are_unsupported() {
        let src = "classDiagram\n  class Box~T~\n";
        let errs = parse(src).expect_err("generics should be unsupported");
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unsupported") && e.message.contains("generic")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn class_self_relation_is_error() {
        let src = "classDiagram\n  A --> A\n";
        let errs = parse(src).expect_err("self relation should be an error");
        assert!(
            errs.iter().any(|e| e.message.contains("self relations")),
            "got: {errs:?}"
        );
    }

    // -----------------------------------------------------------------------
    // ER diagram tests
    // -----------------------------------------------------------------------

    fn er_diagram(d: &Diagram) -> &kozue_ir::ErDiagram {
        match d {
            Diagram::Er(e) => e,
            other => panic!("expected er diagram, got {other:?}"),
        }
    }

    #[test]
    fn er_basic_relation() {
        let src = "erDiagram\n  CUSTOMER ||--o{ ORDER : places\n";
        let d = parse(src).expect("should parse");
        let e = er_diagram(&d);
        assert_eq!(e.entities.len(), 2);
        assert_eq!(e.relations.len(), 1);
        let r = &e.relations[0];
        assert_eq!(r.from, "CUSTOMER");
        assert_eq!(r.to, "ORDER");
        assert_eq!(r.from_marker, EndMarker::ErOne);
        assert_eq!(r.to_marker, EndMarker::ErZeroOrMany);
        assert_eq!(r.line, LineStyle::Solid);
        assert_eq!(r.label.as_deref(), Some("places"));
    }

    #[test]
    fn er_entity_block_with_attributes() {
        let src = r#"erDiagram
  CUSTOMER {
    string name PK
    string email "unique"
  }
"#;
        let d = parse(src).expect("should parse");
        let e = er_diagram(&d);
        let entity = &e.entities["CUSTOMER"];
        assert_eq!(entity.attributes.len(), 2);
        assert_eq!(entity.attributes[0].type_name, "string");
        assert_eq!(entity.attributes[0].name, "name");
        assert_eq!(entity.attributes[0].keys, vec!["PK".to_string()]);
        assert_eq!(entity.attributes[1].comment.as_deref(), Some("unique"));
    }

    #[test]
    fn er_single_line_inline_entity_block() {
        // F3: inline `NAME { a; b }` with `;`-separated attributes.
        let src = "erDiagram\n  ORDER { int id PK; int customer_id FK }\n";
        let d = parse(src).expect("should parse");
        let e = er_diagram(&d);
        let entity = &e.entities["ORDER"];
        assert_eq!(entity.attributes.len(), 2);
        assert_eq!(entity.attributes[0].name, "id");
        assert_eq!(entity.attributes[0].keys, vec!["PK".to_string()]);
        assert_eq!(entity.attributes[1].name, "customer_id");
        assert_eq!(entity.attributes[1].keys, vec!["FK".to_string()]);
    }

    #[test]
    fn er_non_identifying_dashed_relation() {
        let src = "erDiagram\n  A o|..|o B : maybe\n";
        let d = parse(src).expect("should parse");
        let e = er_diagram(&d);
        assert_eq!(e.relations[0].line, LineStyle::Dashed);
    }

    #[test]
    fn er_unrecognised_line_is_error() {
        let src = "erDiagram\n  this is not valid\n";
        let errs = parse(src).expect_err("should error");
        assert!(!errs.is_empty());
    }

    #[test]
    fn er_self_relation_is_error() {
        let src = "erDiagram\n  A ||--|| A : self\n";
        let errs = parse(src).expect_err("self relation should be an error");
        assert!(
            errs.iter().any(|e| e.message.contains("self relations")),
            "got: {errs:?}"
        );
    }
}
