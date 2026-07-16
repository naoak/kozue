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
//! - Node shapes: bare nodes use the legacy unspecified shape; `[label]`,
//!   `(label)`, `((label))`, and `{label}` map to rectangle, rounded rectangle,
//!   circle, and diamond respectively.
//! - Sequence open arrows `->` and `-->` map to `ArrowType::Triangle` with the
//!   same solid/dashed line style as `-->>` / `->>`.
//! - Flowchart directions TD/TB, LR, BT, and RL map to Down, Right, Up, and
//!   Left respectively.
//! - Unsupported features (Note, loop, alt, subgraph, classDef, style, etc.)
//!   are reported as positioned "unsupported" errors rather than crashing or
//!   silently ignoring.

pub mod features;

use std::ops::Range;

use ariadne::{Label, Report, ReportKind, Source};
use indexmap::IndexMap;
use kozue_ir::{
    ArrowType, ClassDiagram, ClassNode, ClassRelation, Container, Diagram, Direction, Edge,
    EndMarker, Endpoint, ErAttribute, ErDiagram, ErEntity, ErRelation, GraphDiagram, IrDocument,
    LineStyle, LineWeight, Message, Node, NodeKind, Participant, SequenceDiagram, SequenceItem,
    State, StateDiagram, Transition,
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
fn parse_diagram(source: &str) -> Result<Diagram, Vec<Diagnostic>> {
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
            "RL" => Direction::Left,
            "BT" => Direction::Up,
            "" => {
                // Mermaid allows omitting direction; default to TD.
                Direction::Down
            }
            _ => {
                errors.push(Diagnostic::new(
                    format!(
                        "unknown flowchart direction `{dir_str}`; expected TD, TB, LR, RL, or BT"
                    ),
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

/// Parse Mermaid source text into a versioned semantic IR document.
///
/// Mermaid does not define a document name in the supported headers, so
/// metadata uses the default empty value.
pub fn parse_document(source: &str) -> Result<IrDocument, Vec<Diagnostic>> {
    parse_diagram(source).map(IrDocument::new)
}

/// Parse Mermaid source text into a semantic [`Diagram`].
pub fn parse(source: &str) -> Result<Diagram, Vec<Diagnostic>> {
    parse_document(source).map(IrDocument::into_diagram)
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
    struct NodeSpec {
        label: String,
        kind: NodeKind,
    }
    let mut node_specs: IndexMap<String, NodeSpec> = IndexMap::new();
    // Raw edges to process after scanning all lines.
    struct RawEdge {
        from: String,
        to: String,
        label: Option<String>,
        arrow: ArrowType,
        from_arrow: ArrowType,
        line: LineStyle,
        weight: LineWeight,
        span: Range<usize>,
    }
    let mut raw_edges: Vec<RawEdge> = Vec::new();

    // Bare references preserve an explicit declaration; later explicit forms update in place.
    // `container_stack`/`membership_resolved` are passed explicitly (rather than
    // captured) so the loop below can also mutate `container_stack` directly
    // for `subgraph`/`end` handling without a borrow conflict.
    let mut ensure_node =
        |id: &str,
         label: Option<&str>,
         kind: NodeKind,
         container_stack: &mut Vec<ContainerBuilder>,
         membership_resolved: &mut std::collections::HashSet<String>| {
            let explicit = kind != NodeKind::Default;
            if let Some(existing) = node_specs.get_mut(id) {
                if explicit {
                    existing.label = label.unwrap_or(id).to_string();
                    existing.kind = kind;
                }
            } else {
                node_specs.insert(
                    id.to_string(),
                    NodeSpec {
                        label: label.unwrap_or(id).to_string(),
                        kind,
                    },
                );
            }
            // First mention wins: a node is a member of the innermost open
            // subgraph at the moment it is first introduced (declared or
            // referenced); later mentions — inside the same, a different, or no
            // subgraph — never reassign it.
            if membership_resolved.insert(id.to_string()) {
                if let Some(top) = container_stack.last_mut() {
                    top.members.push(id.to_string());
                }
            }
        };

    // In-progress subgraph builders, innermost last.
    struct ContainerBuilder {
        id: String,
        label: Option<String>,
        members: Vec<String>,
        children: Vec<Container>,
        span: Range<usize>,
    }
    let mut container_stack: Vec<ContainerBuilder> = Vec::new();
    let mut root_containers: Vec<Container> = Vec::new();
    let mut membership_resolved: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    // (id, span) for every `subgraph` declaration encountered, in source
    // order, for the post-scan duplicate/collision diagnostics.
    let mut all_container_decls: Vec<(String, Range<usize>)> = Vec::new();
    let mut anon_subgraph_counter = 0usize;

    for &(offset, line) in lines {
        let trimmed = line.trim();
        let trimmed_offset = line.find(trimmed).unwrap_or(0);
        let line_end = offset + line.len();
        let span = offset..line_end;

        // `subgraph <id>`, `subgraph <id> [Title]`, or `subgraph <Title with spaces>`.
        if let Some(rest) = strip_keyword_ci(trimmed, "subgraph") {
            let rest = rest.trim();
            let (id, label) = if rest.is_empty() {
                let id = format!("subGraph{anon_subgraph_counter}");
                anon_subgraph_counter += 1;
                (id, None)
            } else {
                parse_subgraph_header(rest)
            };
            all_container_decls.push((id.clone(), span.clone()));
            container_stack.push(ContainerBuilder {
                id,
                label,
                members: Vec::new(),
                children: Vec::new(),
                span: span.clone(),
            });
            continue;
        }
        if trimmed == "end" {
            match container_stack.pop() {
                Some(builder) => {
                    if builder.members.is_empty() && builder.children.is_empty() {
                        errors.push(Diagnostic::new(
                            format!("subgraph `{}` has no members", builder.id),
                            builder.span.clone(),
                        ));
                    }
                    let mut container = Container::new(builder.id.clone(), builder.label.clone());
                    container.members = builder.members.iter().map(|m| m.clone().into()).collect();
                    container.children = builder.children;
                    match container_stack.last_mut() {
                        Some(parent) => parent.children.push(container),
                        None => root_containers.push(container),
                    }
                }
                None => {
                    errors.push(Diagnostic::new(
                        "unmatched `end` (no open subgraph to close)",
                        span,
                    ));
                }
            }
            continue;
        }

        // `direction <dir>` as the sole content of a subgraph body is a
        // per-subgraph direction override, which kozue does not support. Only
        // a recognized direction token is treated as an override; anything
        // else falls through to normal node / edge parsing so a node that
        // happens to be named `direction` behaves the same in and out of a
        // subgraph.
        if !container_stack.is_empty() {
            if let Some(rest) = strip_keyword_ci(trimmed, "direction") {
                if matches!(
                    rest.trim().to_ascii_uppercase().as_str(),
                    "LR" | "RL" | "TB" | "BT" | "TD"
                ) {
                    errors.push(Diagnostic::new(
                        "unsupported: direction (inside subgraph) (kozue does not support this yet)",
                        span,
                    ));
                    continue;
                }
            }
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

        if let Some((delimiter, relative_start)) = unsupported_node_shape_after_id(trimmed) {
            errors.push(Diagnostic::new(
                unsupported_node_shape_message(),
                offset + trimmed_offset + relative_start
                    ..offset + trimmed_offset + relative_start + delimiter.len(),
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
        if let Some(chain) = try_parse_edge_chain(trimmed, offset + trimmed_offset) {
            match chain {
                Ok(edges) => {
                    for edge in edges {
                        ensure_node(
                            &edge.from_id,
                            edge.from_label.as_deref(),
                            edge.from_kind,
                            &mut container_stack,
                            &mut membership_resolved,
                        );
                        ensure_node(
                            &edge.to_id,
                            edge.to_label.as_deref(),
                            edge.to_kind,
                            &mut container_stack,
                            &mut membership_resolved,
                        );
                        raw_edges.push(RawEdge {
                            from: edge.from_id,
                            to: edge.to_id,
                            label: edge.edge_label,
                            arrow: edge.arrow,
                            from_arrow: edge.from_arrow,
                            line: edge.line,
                            weight: edge.weight,
                            span: span.clone(),
                        });
                    }
                }
                Err(diagnostic) => errors.push(diagnostic),
            }
            continue;
        }

        // Try to parse as a standalone node declaration: `A[label]`, `A(label)`, or bare `A`.
        if let Some((id, label, kind)) = try_parse_node_decl(trimmed) {
            ensure_node(
                &id,
                label.as_deref(),
                kind,
                &mut container_stack,
                &mut membership_resolved,
            );
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

    // Any subgraph left open at EOF is missing its closing `end`.
    for builder in &container_stack {
        errors.push(Diagnostic::new(
            format!("subgraph `{}` is missing a closing `end`", builder.id),
            builder.span.clone(),
        ));
    }

    // Duplicate subgraph ids, and subgraph ids colliding with a node id.
    // Checked as a whole-document pass (mirroring the native DSL), since a
    // node or subgraph may be declared either before or after the colliding
    // declaration.
    let mut seen_container_ids: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (id, decl_span) in &all_container_decls {
        if !seen_container_ids.insert(id.as_str()) {
            errors.push(Diagnostic::new(
                format!("duplicate subgraph id `{id}`"),
                decl_span.clone(),
            ));
        }
    }
    for (id, decl_span) in &all_container_decls {
        if node_specs.contains_key(id.as_str()) {
            errors.push(Diagnostic::new(
                format!("subgraph id `{id}` collides with a node of the same name"),
                decl_span.clone(),
            ));
        }
    }

    // Build GraphDiagram.
    let mut graph = GraphDiagram::new(direction);
    for (id, spec) in &node_specs {
        graph.nodes.insert(
            id.clone().into(),
            Node::with_kind(id.clone(), spec.label.clone(), spec.kind.clone()),
        );
    }
    graph.containers = root_containers;

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
        graph.edges.push(Edge::with_presentation(
            re.from.clone(),
            re.to.clone(),
            re.label.clone(),
            re.arrow,
            re.from_arrow,
            re.line,
            re.weight,
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
                .insert(id.into(), Participant::new(id, lbl));
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
            .insert(id.clone().into(), State::new(id.clone(), label.clone()));
    }
    for rt in &transitions {
        for ep in [&rt.from, &rt.to] {
            if let Endpoint::State(id) = ep {
                if !diagram.states.contains_key(id) {
                    diagram
                        .states
                        .insert(id.clone(), State::new(id.clone(), id.to_string()));
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
        Some((id, rest)) if rest.trim().is_empty() => Ok(Endpoint::State(id.into())),
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

/// A fully parsed flowchart edge (one hop of a possibly-chained `A --> B --> C` line).
struct FlowchartEdge {
    from_id: String,
    from_label: Option<String>,
    from_kind: NodeKind,
    to_id: String,
    to_label: Option<String>,
    to_kind: NodeKind,
    edge_label: Option<String>,
    arrow: ArrowType,
    from_arrow: ArrowType,
    line: LineStyle,
    weight: LineWeight,
}

/// (from, to, label, line_style, arrow)
type SeqMsgResult = (String, String, Option<String>, LineStyle, ArrowType);

/// Result of parsing one edge operator+target segment (e.g. the `--> B` part
/// of `A --> B`).
struct EdgeSegment<'a> {
    to_id: String,
    to_label: Option<String>,
    to_kind: NodeKind,
    edge_label: Option<String>,
    arrow: ArrowType,
    from_arrow: ArrowType,
    line: LineStyle,
    weight: LineWeight,
    remainder: &'a str,
}

/// Every flowchart edge operator token, in the order they must be tried:
/// tokens that are a byte-prefix of another token (`-.-` is a prefix of
/// `-.->`) must come after the longer/more-specific one, otherwise the
/// shorter token would shadow it and mis-consume the following `>` as part of
/// the target node. `<-->`/`==>`/`===` don't overlap with anything else but
/// are kept in the same list for a single source of truth.
///
/// `(token, arrow, from_arrow, line, weight)`
const EDGE_OPERATORS: &[(&str, ArrowType, ArrowType, LineStyle, LineWeight)] = &[
    (
        "<-->",
        ArrowType::Triangle,
        ArrowType::Triangle,
        LineStyle::Solid,
        LineWeight::Normal,
    ),
    (
        "-.->",
        ArrowType::Triangle,
        ArrowType::None,
        LineStyle::Dotted,
        LineWeight::Normal,
    ),
    (
        "-.-",
        ArrowType::None,
        ArrowType::None,
        LineStyle::Dotted,
        LineWeight::Normal,
    ),
    (
        "==>",
        ArrowType::Triangle,
        ArrowType::None,
        LineStyle::Solid,
        LineWeight::Thick,
    ),
    (
        "===",
        ArrowType::None,
        ArrowType::None,
        LineStyle::Solid,
        LineWeight::Thick,
    ),
    (
        "-->",
        ArrowType::Triangle,
        ArrowType::None,
        LineStyle::Solid,
        LineWeight::Normal,
    ),
    (
        "---",
        ArrowType::None,
        ArrowType::None,
        LineStyle::Solid,
        LineWeight::Normal,
    ),
];

/// Whether `s` starts with a recognised edge operator token (any of
/// [`EDGE_OPERATORS`], or the `-- label -->`/`-- label ---` space-label form).
fn starts_with_edge_operator(s: &str) -> bool {
    EDGE_OPERATORS
        .iter()
        .any(|(token, ..)| s.starts_with(token))
        || s.starts_with("-- ")
}

/// Parse the text following the `subgraph` keyword into `(id, label)`.
///
/// Mermaid accepts three forms:
/// - `id [Title]` — explicit id with a bracketed display title.
/// - `id` (a single token, no spaces) — the token is both id and (implicit) title.
/// - `Title with spaces` (no brackets) — the whole text becomes the id, with
///   no separate label (mirroring the bare-node convention elsewhere in this
///   frontend, where the id doubles as the label when no explicit title is
///   given).
///
/// `rest` is assumed non-empty and already trimmed.
fn parse_subgraph_header(rest: &str) -> (String, Option<String>) {
    if let Some(bracket_start) = rest.find('[') {
        if rest.ends_with(']') {
            let id_part = rest[..bracket_start].trim();
            let title = rest[bracket_start + 1..rest.len() - 1].trim();
            if !id_part.is_empty() {
                return (id_part.to_string(), Some(title.to_string()));
            }
        }
    }
    (rest.to_string(), None)
}

/// Try to parse a node identifier possibly followed by a shape label: `A[label]`, `A(label)`, or `A`.
///
/// Returns `Some((id, Option<label>))` if the line is a valid standalone node declaration.
/// Returns `None` if the line cannot be a node declaration (e.g. it looks like an edge).
fn try_parse_node_decl(line: &str) -> Option<(String, Option<String>, NodeKind)> {
    // If the line contains any arrow operator it's an edge, not a node.
    if line.contains("-->")
        || line.contains("---")
        || line.contains("->>")
        || line.contains("-->>")
        || line.contains("->")
        || line.contains("-.-")
        || line.contains("==>")
        || line.contains("===")
    {
        return None;
    }
    let (id, rest) = split_id(line)?;
    let rest = rest.trim();
    if rest.is_empty() {
        return Some((id, None, NodeKind::Default));
    }
    if rest.starts_with('[') {
        let (label, after) = extract_bracket_with_rest(rest, '[', ']')?;
        if !after.trim().is_empty() {
            return None;
        }
        return Some((id, Some(label), NodeKind::Rectangle));
    }
    if rest.starts_with("((") {
        let (label, after) = extract_circle_with_rest(rest)?;
        if !after.trim().is_empty() {
            return None;
        }
        return Some((id, Some(label), NodeKind::Circle));
    }
    if rest.starts_with('(') {
        let (label, after) = extract_bracket_with_rest(rest, '(', ')')?;
        if !after.trim().is_empty() {
            return None;
        }
        return Some((id, Some(label), NodeKind::RoundedRectangle));
    }
    if rest.starts_with('{') {
        let (label, after) = extract_bracket_with_rest(rest, '{', '}')?;
        if !after.trim().is_empty() {
            return None;
        }
        return Some((id, Some(label), NodeKind::Diamond));
    }
    None
}

/// Parse one edge operator+target segment from `rest`, returning
/// `(to_id, to_label, edge_label, arrow, remainder_after_to_node)` or None/Err.
enum SegmentParseError {
    Message(String),
    UnsupportedShape {
        relative_start: usize,
        delimiter_len: usize,
    },
}

impl From<String> for SegmentParseError {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

fn unsupported_segment_error(segment: &str, node: &str) -> Option<SegmentParseError> {
    let (delimiter, node_relative_start) = unsupported_node_shape_after_id(node)?;
    let node_start = node.as_ptr() as usize - segment.as_ptr() as usize;
    Some(SegmentParseError::UnsupportedShape {
        relative_start: node_start + node_relative_start,
        delimiter_len: delimiter.len(),
    })
}

fn multi_target_error() -> SegmentParseError {
    "unsupported: multi-target edge (`&`); split into separate edge lines instead"
        .to_string()
        .into()
}

/// Parse the target node (and optional trailing `|label|`) after an edge
/// operator has already been stripped. `rest` is the full segment (operator
/// included) — only used for unsupported-shape span calculations. `op_rest`
/// is everything after the operator token. `token` is the operator token
/// itself, used only for the pipe-label error message.
#[allow(clippy::too_many_arguments)]
fn parse_edge_target<'a>(
    rest: &'a str,
    op_rest: &'a str,
    token: &str,
    arrow: ArrowType,
    from_arrow: ArrowType,
    line: LineStyle,
    weight: LineWeight,
) -> Option<Result<EdgeSegment<'a>, SegmentParseError>> {
    let op_rest = op_rest.trim_start();

    if op_rest.starts_with('|') {
        // `<op>|label| to_node`
        let (edge_label, rest3) = extract_pipe_label(op_rest)?;
        let rest3 = rest3.trim_start();
        if let Some(error) = unsupported_segment_error(rest, rest3) {
            return Some(Err(error));
        }
        let (to_id, to_label, to_kind, after) = match parse_node_with_label(rest3) {
            Some(r) => r,
            None => {
                return Some(Err(format!(
                    "syntax error: expected node identifier after `{token}|{edge_label}|`, got `{}`",
                    rest3.chars().take(20).collect::<String>()
                )
                .into()));
            }
        };
        let after = after.trim_start();
        if after.starts_with('&') {
            return Some(Err(multi_target_error()));
        }
        return Some(Ok(EdgeSegment {
            to_id,
            to_label,
            to_kind,
            edge_label: Some(edge_label),
            arrow,
            from_arrow,
            line,
            weight,
            remainder: after,
        }));
    }

    // Check for `&` (multi-target) before to-node.
    if op_rest.starts_with('&') {
        return Some(Err(multi_target_error()));
    }
    if let Some(error) = unsupported_segment_error(rest, op_rest) {
        return Some(Err(error));
    }
    let (to_id, to_label, to_kind, after) = parse_node_with_label(op_rest)?;
    let after = after.trim_start();
    if after.starts_with('&') {
        return Some(Err(multi_target_error()));
    }
    Some(Ok(EdgeSegment {
        to_id,
        to_label,
        to_kind,
        edge_label: None,
        arrow,
        from_arrow,
        line,
        weight,
        remainder: after,
    }))
}

fn parse_one_edge_segment(rest: &str) -> Option<Result<EdgeSegment<'_>, SegmentParseError>> {
    let rest = rest.trim_start();

    // Check for multi-target `&` — must error explicitly.
    if rest.starts_with('&') {
        return Some(Err(multi_target_error()));
    }

    // Try each single-token operator, longest/most-specific first (see
    // `EDGE_OPERATORS` docs for why order matters).
    for &(token, arrow, from_arrow, line, weight) in EDGE_OPERATORS {
        if let Some(op_rest) = rest.strip_prefix(token) {
            return parse_edge_target(rest, op_rest, token, arrow, from_arrow, line, weight);
        }
    }

    // Try `-- label -->` / `-- label ---` (space-label form; legacy operators
    // only — Mermaid's middle-label form is not extended to the new dotted /
    // thick / bidirectional operators this milestone).
    if let Some(rest2) = rest.strip_prefix("-- ") {
        if let Some(arrow_idx) = rest2.find("-->") {
            let label = rest2[..arrow_idx].trim().to_string();
            let rest3 = &rest2[arrow_idx + 3..];
            let edge_label = if label.is_empty() { None } else { Some(label) };
            return parse_edge_target(
                rest,
                rest3,
                "-->",
                ArrowType::Triangle,
                ArrowType::None,
                LineStyle::Solid,
                LineWeight::Normal,
            )
            .map(|result| {
                result.map(|segment| EdgeSegment {
                    edge_label,
                    ..segment
                })
            });
        }
        if let Some(arrow_idx) = rest2.find("---") {
            let label = rest2[..arrow_idx].trim().to_string();
            let rest3 = &rest2[arrow_idx + 3..];
            let edge_label = if label.is_empty() { None } else { Some(label) };
            return parse_edge_target(
                rest,
                rest3,
                "---",
                ArrowType::None,
                ArrowType::None,
                LineStyle::Solid,
                LineWeight::Normal,
            )
            .map(|result| {
                result.map(|segment| EdgeSegment {
                    edge_label,
                    ..segment
                })
            });
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
    offset: usize,
) -> Option<Result<Vec<FlowchartEdge>, Diagnostic>> {
    if let Some((delimiter, relative_start)) = unsupported_node_shape_after_id(line) {
        return Some(Err(Diagnostic::new(
            unsupported_node_shape_message(),
            offset + relative_start..offset + relative_start + delimiter.len(),
        )));
    }
    let (first_id, first_label, first_kind, rest) = parse_node_with_label(line)?;
    let rest = rest.trim_start();

    // Must look like an edge (starts with an operator).
    if !starts_with_edge_operator(rest) {
        return None;
    }

    let mut results: Vec<FlowchartEdge> = Vec::new();
    let mut from_id = first_id;
    let mut from_label = first_label;
    let mut from_kind = first_kind;
    let mut current_rest = rest;

    loop {
        match parse_one_edge_segment(current_rest) {
            None => {
                // No operator recognised — if we already parsed at least one edge,
                // any non-empty remainder is an error.
                if current_rest.is_empty() {
                    break;
                }
                return Some(Err(Diagnostic::new(
                    format!(
                        "syntax error: unexpected tokens in edge: `{}`",
                        current_rest.chars().take(40).collect::<String>()
                    ),
                    offset..offset + line.len(),
                )));
            }
            Some(Err(SegmentParseError::Message(message))) => {
                return Some(Err(Diagnostic::new(message, offset..offset + line.len())))
            }
            Some(Err(SegmentParseError::UnsupportedShape {
                relative_start,
                delimiter_len,
            })) => {
                let segment_start = current_rest.as_ptr() as usize - line.as_ptr() as usize;
                let start = offset + segment_start + relative_start;
                return Some(Err(Diagnostic::new(
                    unsupported_node_shape_message(),
                    start..start + delimiter_len,
                )));
            }
            Some(Ok(segment)) => {
                results.push(FlowchartEdge {
                    from_id: from_id.clone(),
                    from_label: from_label.clone(),
                    from_kind: from_kind.clone(),
                    to_id: segment.to_id.clone(),
                    to_label: segment.to_label.clone(),
                    to_kind: segment.to_kind.clone(),
                    edge_label: segment.edge_label,
                    arrow: segment.arrow,
                    from_arrow: segment.from_arrow,
                    line: segment.line,
                    weight: segment.weight,
                });
                from_id = segment.to_id;
                from_label = segment.to_label;
                from_kind = segment.to_kind;
                current_rest = segment.remainder.trim_start();
                if current_rest.is_empty() {
                    break;
                }
                // If remainder doesn't start with an operator, it's an error.
                if !starts_with_edge_operator(current_rest) {
                    return Some(Err(Diagnostic::new(
                        format!(
                            "syntax error: unexpected tokens after edge: `{}`",
                            current_rest.chars().take(40).collect::<String>()
                        ),
                        offset..offset + line.len(),
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

fn unsupported_node_shape_message() -> String {
    "unsupported: this compound Mermaid node shape is not supported by kozue".to_string()
}

fn unsupported_node_shape_after_id(s: &str) -> Option<(&'static str, usize)> {
    let start = s.as_ptr() as usize;
    let (_, rest) = split_id(s)?;
    let rest = rest.trim_start();
    let delimiter = ["(((", "{{", "@{", "([", "[[", "[(", "[/", "[\\"]
        .into_iter()
        .find(|delimiter| rest.starts_with(delimiter))?;
    Some((delimiter, rest.as_ptr() as usize - start))
}

fn extract_circle_with_rest(s: &str) -> Option<(String, &str)> {
    let (content, rest) = extract_bracket_with_rest(s, '(', ')')?;
    let label = content.strip_prefix('(')?.strip_suffix(')')?.to_string();
    Some((label, rest))
}

/// Parse a node reference at the start of `s`, which may have an optional shape
/// label (`[label]` or `(label)`). Returns `(id, Option<label>, rest)` or None.
///
/// Unsupported compound shapes are rejected by the caller before this parser.
fn parse_node_with_label(s: &str) -> Option<(String, Option<String>, NodeKind, &str)> {
    let (id, rest) = split_id(s)?;
    let rest = rest.trim_start();
    if rest.starts_with('[') {
        let (label, after) = extract_bracket_with_rest(rest, '[', ']')?;
        return Some((id, Some(label), NodeKind::Rectangle, after));
    }
    // Reject stadium `([` before the generic rounded-rectangle parser.
    if rest.starts_with("([") {
        return None;
    }
    if rest.starts_with("((") {
        let (label, after) = extract_circle_with_rest(rest)?;
        return Some((id, Some(label), NodeKind::Circle, after));
    }
    if rest.starts_with('(') {
        let (label, after) = extract_bracket_with_rest(rest, '(', ')')?;
        return Some((id, Some(label), NodeKind::RoundedRectangle, after));
    }
    if rest.starts_with('{') {
        let (label, after) = extract_bracket_with_rest(rest, '{', '}')?;
        return Some((id, Some(label), NodeKind::Diamond, after));
    }
    Some((id, None, NodeKind::Default, rest))
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

fn extract_bracket_with_rest(s: &str, open: char, close: char) -> Option<(String, &str)> {
    let s = s.trim_start();
    if !s.starts_with(open) {
        return None;
    }
    let mut depth = 0usize;
    let mut content = String::new();
    let mut started = false;
    for (index, c) in s.char_indices() {
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
                return Some((content, &s[index + c.len_utf8()..]));
            }
            content.push(c);
        } else {
            content.push(c);
        }
    }
    None
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
                id.to_string().into(),
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
                        diagram.classes[id.as_str()].stereotype = Some(stereotype);
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
    } else {
        (tok.find("..")?, true)
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
                id.to_string().into(),
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
                        Ok(a) => diagram.entities[id.as_str()].attributes.push(a),
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
                    Ok(attr) => diagram.entities[id.as_str()].attributes.push(attr),
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

    #[test]
    fn parse_document_has_no_mermaid_name() {
        let document = parse_document("flowchart TD\n  A --> B\n").unwrap();
        assert_eq!(document.metadata.name, None);
        assert!(document.extensions.is_empty());
        assert_eq!(
            parse("flowchart TD\n  A --> B\n").unwrap(),
            document.into_diagram()
        );
    }

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
    fn last_explicit_label_wins() {
        let src = "flowchart TD\n  A[First] --> B\n  A[Second] --> C\n";
        let d = parse(src).expect("should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.nodes["A"].label, "Second");
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
    fn dotted_directed_edge() {
        let src = "flowchart TD\n  A -.-> B\n";
        let d = parse(src).expect("should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.edges[0].arrow, ArrowType::Triangle);
        assert_eq!(g.edges[0].from_arrow, ArrowType::None);
        assert_eq!(g.edges[0].line, LineStyle::Dotted);
        assert_eq!(g.edges[0].weight, LineWeight::Normal);
    }

    #[test]
    fn dotted_directed_edge_with_pipe_label() {
        let src = "flowchart TD\n  A -.->|maybe| B\n";
        let d = parse(src).expect("should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.edges[0].label.as_deref(), Some("maybe"));
        assert_eq!(g.edges[0].line, LineStyle::Dotted);
        assert_eq!(g.edges[0].arrow, ArrowType::Triangle);
    }

    #[test]
    fn dotted_undirected_edge() {
        let src = "flowchart TD\n  A -.- B\n";
        let d = parse(src).expect("should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.edges[0].arrow, ArrowType::None);
        assert_eq!(g.edges[0].from_arrow, ArrowType::None);
        assert_eq!(g.edges[0].line, LineStyle::Dotted);
    }

    #[test]
    fn dotted_undirected_edge_with_pipe_label() {
        let src = "flowchart TD\n  A -.-|note| B\n";
        let d = parse(src).expect("should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.edges[0].label.as_deref(), Some("note"));
        assert_eq!(g.edges[0].arrow, ArrowType::None);
        assert_eq!(g.edges[0].line, LineStyle::Dotted);
    }

    #[test]
    fn thick_directed_edge() {
        let src = "flowchart TD\n  A ==> B\n";
        let d = parse(src).expect("should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.edges[0].arrow, ArrowType::Triangle);
        assert_eq!(g.edges[0].line, LineStyle::Solid);
        assert_eq!(g.edges[0].weight, LineWeight::Thick);
    }

    #[test]
    fn thick_directed_edge_with_pipe_label() {
        let src = "flowchart TD\n  A ==>|go| B\n";
        let d = parse(src).expect("should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.edges[0].label.as_deref(), Some("go"));
        assert_eq!(g.edges[0].weight, LineWeight::Thick);
    }

    #[test]
    fn thick_undirected_edge() {
        let src = "flowchart TD\n  A === B\n";
        let d = parse(src).expect("should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.edges[0].arrow, ArrowType::None);
        assert_eq!(g.edges[0].weight, LineWeight::Thick);
    }

    #[test]
    fn thick_undirected_edge_with_pipe_label() {
        let src = "flowchart TD\n  A ===|link| B\n";
        let d = parse(src).expect("should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.edges[0].label.as_deref(), Some("link"));
        assert_eq!(g.edges[0].arrow, ArrowType::None);
        assert_eq!(g.edges[0].weight, LineWeight::Thick);
    }

    #[test]
    fn bidirectional_edge() {
        let src = "flowchart TD\n  A <--> B\n";
        let d = parse(src).expect("should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.edges[0].arrow, ArrowType::Triangle);
        assert_eq!(g.edges[0].from_arrow, ArrowType::Triangle);
        assert_eq!(g.edges[0].line, LineStyle::Solid);
        assert_eq!(g.edges[0].weight, LineWeight::Normal);
    }

    #[test]
    fn bidirectional_edge_with_pipe_label() {
        let src = "flowchart TD\n  A <-->|both| B\n";
        let d = parse(src).expect("should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.edges[0].label.as_deref(), Some("both"));
        assert_eq!(g.edges[0].arrow, ArrowType::Triangle);
        assert_eq!(g.edges[0].from_arrow, ArrowType::Triangle);
    }

    #[test]
    fn dotted_undirected_is_not_shadowed_by_dotted_directed_prefix_overlap() {
        // `-.-` is a byte-prefix of `-.->`; both forms must resolve correctly.
        let src = "flowchart TD\n  A -.- B\n  C -.-> D\n";
        let d = parse(src).expect("should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.edges[0].arrow, ArrowType::None);
        assert_eq!(g.edges[0].line, LineStyle::Dotted);
        assert_eq!(g.edges[1].arrow, ArrowType::Triangle);
        assert_eq!(g.edges[1].line, LineStyle::Dotted);
    }

    #[test]
    fn thick_undirected_is_not_shadowed_by_thick_directed_prefix_overlap() {
        let src = "flowchart TD\n  A === B\n  C ==> D\n";
        let d = parse(src).expect("should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.edges[0].arrow, ArrowType::None);
        assert_eq!(g.edges[0].weight, LineWeight::Thick);
        assert_eq!(g.edges[1].arrow, ArrowType::Triangle);
        assert_eq!(g.edges[1].weight, LineWeight::Thick);
    }

    #[test]
    fn bidirectional_is_not_shadowed_by_directed_arrow() {
        let src = "flowchart TD\n  A <--> B\n  C --> D\n";
        let d = parse(src).expect("should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.edges[0].from_arrow, ArrowType::Triangle);
        assert_eq!(g.edges[1].from_arrow, ArrowType::None);
    }

    #[test]
    fn new_edge_operators_are_recognised_in_chains() {
        let src = "flowchart TD\n  A -.-> B ==> C\n";
        let d = parse(src).expect("should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.edges.len(), 2);
        assert_eq!(g.edges[0].line, LineStyle::Dotted);
        assert_eq!(g.edges[1].weight, LineWeight::Thick);
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
    fn direction_rl_is_left() {
        let src = "graph RL\n  A --> B\n";
        let Diagram::Graph(graph) = parse(src).expect("RL should parse") else {
            panic!("expected graph")
        };
        assert_eq!(graph.direction, Direction::Left);
    }

    #[test]
    fn direction_bt_is_up() {
        let src = "flowchart BT\n  A --> B\n";
        let Diagram::Graph(graph) = parse(src).expect("BT should parse") else {
            panic!("expected graph")
        };
        assert_eq!(graph.direction, Direction::Up);
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
    fn supported_node_shapes_map_to_semantic_kinds() {
        let src = "flowchart TD\n  A\n  B[rectangle]\n  C(rounded)\n  D((circle))\n  E{diamond}\n";
        let d = parse(src).expect("node shapes should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.nodes["A"].kind, NodeKind::Default);
        assert_eq!(g.nodes["B"].kind, NodeKind::Rectangle);
        assert_eq!(g.nodes["C"].kind, NodeKind::RoundedRectangle);
        assert_eq!(g.nodes["D"].kind, NodeKind::Circle);
        assert_eq!(g.nodes["E"].kind, NodeKind::Diamond);
    }

    #[test]
    fn standalone_node_shapes_reject_trailing_tokens() {
        for declaration in [
            "A[rect] garbage",
            "A(round) garbage",
            "A((circle)) garbage",
            "A{diamond} garbage",
        ] {
            let source = format!("flowchart TD\n  {declaration}\n");
            let errors = parse(&source).expect_err("trailing tokens must be rejected");
            assert!(
                errors
                    .iter()
                    .any(|error| error.message.contains("syntax error")),
                "got: {errors:?}"
            );
        }
    }

    #[test]
    fn unsupported_long_node_delimiters_have_exact_spans_standalone_and_on_edges() {
        for (shape, delimiter) in [
            ("A(((double)))", "((("),
            ("A{{hexagon}}", "{{"),
            ("A@{ shape: circle }", "@{"),
        ] {
            for statement in [shape.to_string(), format!("X --> {shape}")] {
                let source = format!("flowchart TD\n  {statement}\n");
                let errors = parse(&source).expect_err("shape must be unsupported");
                let start = source.find(delimiter).unwrap();
                assert!(
                    errors.iter().any(|error| {
                        error.message.contains("unsupported")
                            && error.span == (start..start + delimiter.len())
                    }),
                    "expected exact span for {statement:?}, got: {errors:?}"
                );
            }
        }
    }

    #[test]
    fn explicit_declarations_update_in_place_and_bare_references_do_not_overwrite() {
        let src = "flowchart TD\n  A[first] --> B(round) --> C\n  A --> D\n  A((last)) --> E\n  B --> E\n  B[later] --> E\n";
        let Diagram::Graph(graph) = parse(src).expect("flowchart should parse") else {
            panic!("expected graph")
        };
        assert_eq!(graph.nodes["A"].label, "last");
        assert_eq!(graph.nodes["A"].kind, NodeKind::Circle);
        assert_eq!(graph.nodes["B"].label, "later");
        assert_eq!(graph.nodes["B"].kind, NodeKind::Rectangle);
        assert_eq!(graph.nodes["C"].kind, NodeKind::Default);
        assert_eq!(
            graph.nodes.keys().map(|id| id.as_str()).collect::<Vec<_>>(),
            ["A", "B", "C", "D", "E"]
        );
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
        assert_eq!(m.from.as_str(), "A");
        assert_eq!(m.to.as_str(), "A");
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
        let src = "flowchart TD\n  classDef foo fill:red\n  A --> B\n  end\n  style A fill:red\n";
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
        assert_eq!(g.edges[0].from.as_str(), "A");
        assert_eq!(g.edges[0].to.as_str(), "B");
        assert_eq!(g.edges[1].from.as_str(), "B");
        assert_eq!(g.edges[1].to.as_str(), "C");
    }

    #[test]
    fn chain_four_nodes() {
        let src = "flowchart TD\n  A --> B --> C --> D\n";
        let d = parse(src).expect("four-node chain should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.edges.len(), 3);
        assert_eq!(g.edges[0].from.as_str(), "A");
        assert_eq!(g.edges[2].to.as_str(), "D");
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
        assert_eq!(g.edges[1].from.as_str(), "B");
        assert_eq!(g.edges[1].to.as_str(), "C");
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
                        e.to.as_str(),
                        "b",
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
            errs.iter().any(|e| e.message.contains("unsupported")),
            "got: {errs:?}"
        );
        let start = src.find("([").unwrap();
        assert!(errs.iter().any(|error| error.span == (start..start + 2)));
    }

    #[test]
    fn circle_and_diamond_are_supported_at_edge_endpoints() {
        let source = "flowchart TD\n  A{decision} --> B((circle))\n";
        let Diagram::Graph(graph) = parse(source).expect("shapes should parse") else {
            panic!("expected graph")
        };
        assert_eq!(graph.nodes["A"].kind, NodeKind::Diamond);
        assert_eq!(graph.nodes["A"].label, "decision");
        assert_eq!(graph.nodes["B"].kind, NodeKind::Circle);
        assert_eq!(graph.nodes["B"].label, "circle");
    }

    #[test]
    fn remaining_compound_node_shapes_are_unsupported() {
        for source in [
            "flowchart TD\n  A --> B[[subroutine]]\n",
            "flowchart TD\n  A --> B[(database)]\n",
        ] {
            let errors = parse(source).expect_err("shape should be unsupported");
            assert!(errors
                .iter()
                .any(|error| error.message.contains("unsupported")));
        }
    }

    #[test]
    fn compound_shape_tokens_inside_supported_labels_are_allowed() {
        let src = "flowchart TD\n  A[text ([ok])] --> B\n  B -->|Map {key} [[value]]| C\n";
        let Diagram::Graph(graph) = parse(src).expect("label text should not select a shape")
        else {
            panic!("expected graph")
        };
        assert_eq!(graph.nodes["A"].label, "text ([ok])");
        assert_eq!(graph.nodes["A"].kind, NodeKind::Rectangle);
        assert_eq!(graph.edges[1].label.as_deref(), Some("Map {key} [[value]]"));
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
        assert_eq!(c.relations[0].from.as_str(), "Dog");
        assert_eq!(c.relations[0].to.as_str(), "Animal");
        assert_eq!(c.relations[0].from_marker, EndMarker::HollowTriangle);
        assert_eq!(c.relations[0].to_marker, EndMarker::None);
    }

    #[test]
    fn class_mermaid_direction_inheritance() {
        let src = "classDiagram\n  Dog <|-- Animal\n";
        let d = parse(src).expect("should parse");
        let c = class_diagram(&d);
        assert_eq!(c.relations[0].from.as_str(), "Dog");
        assert_eq!(c.relations[0].to.as_str(), "Animal");
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
            assert_eq!(r.from.as_str(), "A", "`{line}` from");
            assert_eq!(r.to.as_str(), "B", "`{line}` to");
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
        assert_eq!(r.from.as_str(), "CUSTOMER");
        assert_eq!(r.to.as_str(), "ORDER");
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

    // -----------------------------------------------------------------
    // M3a3 Phase 2.2: subgraph / container (Mermaid)
    // -----------------------------------------------------------------

    #[test]
    fn subgraph_basic_builds_container() {
        // `B` is first mentioned at the top level (in `A --> B`), so it is
        // *not* a member of `one` despite being re-mentioned inside it —
        // only `C`, first mentioned inside the subgraph, is a member.
        let src = "flowchart TD\n  A --> B\n  subgraph one\n    B --> C\n  end\n";
        let Diagram::Graph(g) = parse(src).expect("should parse") else {
            panic!("expected graph")
        };
        assert_eq!(g.nodes.len(), 3);
        assert_eq!(g.containers.len(), 1);
        let c = &g.containers[0];
        assert_eq!(c.id.as_str(), "one");
        assert_eq!(c.label, None);
        assert_eq!(
            c.members.iter().map(|m| m.as_str()).collect::<Vec<_>>(),
            vec!["C"]
        );
        assert!(c.children.is_empty());
    }

    #[test]
    fn subgraph_nested_builds_container_tree() {
        let src = "flowchart TD\n  subgraph outer\n    A --> B\n    subgraph inner\n      B --> C\n    end\n  end\n";
        let Diagram::Graph(g) = parse(src).expect("should parse") else {
            panic!("expected graph")
        };
        assert_eq!(g.containers.len(), 1);
        let outer = &g.containers[0];
        assert_eq!(outer.id.as_str(), "outer");
        assert_eq!(
            outer.members.iter().map(|m| m.as_str()).collect::<Vec<_>>(),
            vec!["A", "B"]
        );
        assert_eq!(outer.children.len(), 1);
        let inner = &outer.children[0];
        assert_eq!(inner.id.as_str(), "inner");
        assert_eq!(
            inner.members.iter().map(|m| m.as_str()).collect::<Vec<_>>(),
            vec!["C"]
        );
    }

    #[test]
    fn subgraph_title_forms() {
        // `id [Title]`
        let src = "flowchart TD\n  subgraph one [My Title]\n    A\n  end\n";
        let Diagram::Graph(g) = parse(src).expect("should parse") else {
            panic!("expected graph")
        };
        assert_eq!(g.containers[0].id.as_str(), "one");
        assert_eq!(g.containers[0].label.as_deref(), Some("My Title"));

        // bare single-token id (no brackets).
        let src2 = "flowchart TD\n  subgraph one\n    A\n  end\n";
        let Diagram::Graph(g2) = parse(src2).expect("should parse") else {
            panic!("expected graph")
        };
        assert_eq!(g2.containers[0].id.as_str(), "one");
        assert_eq!(g2.containers[0].label, None);

        // bare multiword title (no brackets) — the whole text becomes the id.
        let src3 = "flowchart TD\n  subgraph Multi Word Title\n    A\n  end\n";
        let Diagram::Graph(g3) = parse(src3).expect("should parse") else {
            panic!("expected graph")
        };
        assert_eq!(g3.containers[0].id.as_str(), "Multi Word Title");
        assert_eq!(g3.containers[0].label, None);
    }

    #[test]
    fn subgraph_first_mention_wins_membership() {
        // `A` is first mentioned at the top level, before any subgraph opens;
        // its later re-mention inside `one` must not reassign it.
        let src = "flowchart TD\n  A --> B\n  subgraph one\n    A --> C\n  end\n";
        let Diagram::Graph(g) = parse(src).expect("should parse") else {
            panic!("expected graph")
        };
        assert_eq!(
            g.containers[0]
                .members
                .iter()
                .map(|m| m.as_str())
                .collect::<Vec<_>>(),
            vec!["C"]
        );
    }

    #[test]
    fn subgraph_remention_in_another_subgraph_does_not_reassign() {
        // `B` is first mentioned inside `one`; a later mention inside `two`
        // must not move it, and must not be an error.
        let src = "flowchart TD\n  subgraph one\n    A --> B\n  end\n  subgraph two\n    B --> C\n  end\n";
        let Diagram::Graph(g) = parse(src).expect("should parse") else {
            panic!("expected graph")
        };
        let one = g
            .containers
            .iter()
            .find(|c| c.id.as_str() == "one")
            .unwrap();
        let two = g
            .containers
            .iter()
            .find(|c| c.id.as_str() == "two")
            .unwrap();
        assert!(one.members.iter().any(|m| m.as_str() == "B"));
        assert!(!two.members.iter().any(|m| m.as_str() == "B"));
        assert!(two.members.iter().any(|m| m.as_str() == "C"));
    }

    #[test]
    fn subgraph_unmatched_end_is_error() {
        let src = "flowchart TD\n  A --> B\n  end\n";
        let errs = parse(src).expect_err("unmatched end should fail");
        assert!(errs.iter().any(|e| e.message.contains("unmatched `end`")));
    }

    #[test]
    fn subgraph_unclosed_at_eof_is_error() {
        let src = "flowchart TD\n  subgraph one\n    A --> B\n";
        let errs = parse(src).expect_err("unclosed subgraph should fail");
        assert!(errs
            .iter()
            .any(|e| e.message.contains("missing a closing `end`")));
    }

    #[test]
    fn subgraph_empty_is_error() {
        let src = "flowchart TD\n  A\n  subgraph one\n  end\n";
        let errs = parse(src).expect_err("empty subgraph should fail");
        assert!(errs.iter().any(|e| e.message.contains("has no members")));
    }

    #[test]
    fn subgraph_direction_inside_is_unsupported() {
        let src = "flowchart TD\n  subgraph one\n    direction LR\n    A --> B\n  end\n";
        let errs = parse(src).expect_err("direction inside subgraph should be unsupported");
        assert!(errs
            .iter()
            .any(|e| e.message.contains("unsupported") && e.message.contains("direction")));
    }

    #[test]
    fn subgraph_node_named_direction_is_not_a_direction_override() {
        // Only `direction <LR|RL|TB|BT|TD>` is the per-subgraph override; a
        // node that happens to be named `direction` parses the same in and
        // out of a subgraph.
        let src = "flowchart TD\n  subgraph one\n    direction --> B\n  end\n";
        let Diagram::Graph(graph) = parse(src).expect("should parse") else {
            panic!("expected graph")
        };
        assert!(graph
            .nodes
            .contains_key(&kozue_ir::ElementId::from("direction")));
        assert_eq!(graph.containers.len(), 1);
        assert_eq!(
            graph.containers[0]
                .members
                .iter()
                .map(|m| m.as_str())
                .collect::<Vec<_>>(),
            vec!["direction", "B"]
        );
    }

    #[test]
    fn subgraph_collision_diagnostics() {
        // duplicate subgraph id.
        let src = "flowchart TD\n  subgraph one\n    A\n  end\n  subgraph one\n    B\n  end\n";
        let errs = parse(src).expect_err("duplicate subgraph id should fail");
        assert!(errs
            .iter()
            .any(|e| e.message.contains("duplicate subgraph id")));

        // subgraph id colliding with a node id.
        let src2 = "flowchart TD\n  one --> B\n  subgraph one\n    C\n  end\n";
        let errs2 = parse(src2).expect_err("subgraph/node collision should fail");
        assert!(errs2
            .iter()
            .any(|e| e.message.contains("collides with a node")));
    }
}
