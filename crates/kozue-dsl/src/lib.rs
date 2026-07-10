//! DSL parser for kozue using chumsky 0.9, with ariadne diagnostics.
//!
//! Grammar (M0 + M2a):
//! ```text
//! diagram <name> {
//!   direction down|right
//!   <id>: "label"
//!   <a> -> <b> : "label"
//! }
//!
//! diagram <name> {
//!   participant <id>: "label"
//!   <a> -> <b> : "label"
//!   <a> --> <b> : "label"
//! }
//! ```

use ariadne::{Label, Report, ReportKind, Source};
use chumsky::prelude::*;
use kozue_ir::{
    ArrowType, Diagram, Direction, Edge, GraphDiagram, LineStyle, Message, Node, Participant,
    SequenceDiagram, SequenceItem,
};

/// A parsed statement inside a diagram body.
#[derive(Debug, Clone)]
enum Stmt {
    Direction(Direction, std::ops::Range<usize>),
    Node {
        id: String,
        id_span: std::ops::Range<usize>,
        label: Option<String>,
    },
    Edge(EdgeStmt),
    DashedEdge(EdgeStmt),
    Participant {
        id: String,
        id_span: std::ops::Range<usize>,
        label: Option<String>,
    },
    DirectionError(std::ops::Range<usize>),
}

#[derive(Debug, Clone)]
struct EdgeStmt {
    from: String,
    from_span: std::ops::Range<usize>,
    to: String,
    to_span: std::ops::Range<usize>,
    label: Option<String>,
}

#[derive(Debug, Clone)]
struct Ast {
    #[allow(dead_code)]
    name: String,
    stmts: Vec<Stmt>,
}

/// A user-facing error with a source span, for pretty diagnostics.
#[derive(Debug, Clone)]
pub struct CompileError {
    pub message: String,
    pub span: std::ops::Range<usize>,
}

fn ident() -> impl Parser<char, String, Error = Simple<char>> + Clone {
    text::ident().padded()
}

fn ident_spanned(
) -> impl Parser<char, (String, std::ops::Range<usize>), Error = Simple<char>> + Clone {
    text::ident().padded().map_with_span(|s, span| (s, span))
}

/// Parse a string literal with escape sequences: `\"` and `\\`.
/// Any other `\x` is a diagnostic error yielded as a fallback character.
fn string_lit() -> impl Parser<char, String, Error = Simple<char>> + Clone {
    // A single character inside a string: normal char, or escape sequence.
    let char_inner = just('\\')
        .ignore_then(
            just('"')
                .to('"')
                .or(just('\\').to('\\'))
                // Any other escaped char: we keep it as-is during parsing;
                // the semantic pass will validate. We use a placeholder so the
                // parser doesn't fail here — validation happens in build_diagram.
                .or(none_of("\"").map(|c: char| c)),
        )
        .or(none_of("\"\\"));

    just('"')
        .ignore_then(char_inner.repeated())
        .then_ignore(just('"'))
        .collect::<String>()
        .padded()
}

fn parser() -> impl Parser<char, Ast, Error = Simple<char>> {
    // direction statement: `direction down|right`
    // If `direction` keyword is consumed but the value is not `down` or `right`,
    // produce a DirectionError (no backtrack to node parser).
    let direction_kw = text::keyword("direction").padded();
    let direction_val = text::keyword("down")
        .to(Direction::Down)
        .or(text::keyword("right").to(Direction::Right));

    let direction = direction_kw
        .ignore_then(
            direction_val
                .map_with_span(Stmt::Direction)
                // If the value is not `down` or `right`, consume a single
                // identifier (or nothing before `}`) and emit an error.
                // We use `text::ident().padded()` so we consume at most
                // one token, preventing the body from being consumed wholesale.
                .or(text::ident()
                    .padded()
                    .map_with_span(|_, span| Stmt::DirectionError(span)))
                .or(empty().map_with_span(|_, span| Stmt::DirectionError(span))),
        )
        .map_with_span(|s, _span| s);

    // participant: `participant id` or `participant id: "label"`
    let participant = text::keyword("participant")
        .padded()
        .ignore_then(ident_spanned())
        .then(just(':').padded().ignore_then(string_lit()).or_not())
        .map(|((id, id_span), label)| Stmt::Participant { id, id_span, label });

    // Dashed edge: `a --> b` optionally `: "label"`
    // IMPORTANT: must be tried BEFORE solid edge `->` so `-->` is not parsed as `->` + `>`
    let dashed_edge = ident_spanned()
        .then_ignore(just("-->").padded())
        .then(ident_spanned())
        .then(just(':').padded().ignore_then(string_lit()).or_not())
        .map(|(((from, from_span), (to, to_span)), label)| {
            Stmt::DashedEdge(EdgeStmt {
                from,
                from_span,
                to,
                to_span,
                label,
            })
        });

    // Solid edge: `a -> b` optionally `: "label"`.
    let edge = ident_spanned()
        .then_ignore(just("->").padded())
        .then(ident_spanned())
        .then(just(':').padded().ignore_then(string_lit()).or_not())
        .map(|(((from, from_span), (to, to_span)), label)| {
            Stmt::Edge(EdgeStmt {
                from,
                from_span,
                to,
                to_span,
                label,
            })
        });

    // Node: `id : "label"` or `id`. Distinguished from an edge because edges
    // contain `->`.
    let node = ident_spanned()
        .then(just(':').padded().ignore_then(string_lit()).or_not())
        .map(|((id, id_span), label)| Stmt::Node { id, id_span, label });

    // Order matters: try direction first (consumes keyword without backtrack),
    // then participant, then dashed_edge (before solid edge!), then edge, then node.
    let stmt = direction.or(participant).or(dashed_edge).or(edge).or(node);

    let body = stmt.repeated().padded();

    text::keyword("diagram")
        .padded()
        .ignore_then(ident())
        .then_ignore(just('{').padded())
        .then(body)
        .then_ignore(just('}').padded())
        .then_ignore(end())
        .map(|(name, stmts)| Ast { name, stmts })
}

/// Parse source text into a semantic [`Diagram`], collecting errors.
pub fn parse(src: &str) -> Result<Diagram, Vec<CompileError>> {
    let ast = parser().parse(src).map_err(|errs| {
        errs.into_iter()
            .map(|e| CompileError {
                message: format!("{}", e),
                span: e.span(),
            })
            .collect::<Vec<_>>()
    })?;

    build_diagram(ast, src)
}

/// Semantic pass: detect diagram kind and dispatch to the appropriate builder.
fn build_diagram(ast: Ast, src: &str) -> Result<Diagram, Vec<CompileError>> {
    let has_participant = ast
        .stmts
        .iter()
        .any(|s| matches!(s, Stmt::Participant { .. }));
    let has_node = ast.stmts.iter().any(|s| matches!(s, Stmt::Node { .. }));

    if has_participant && has_node {
        // Find the first offending node stmt for a good error span.
        let span = ast
            .stmts
            .iter()
            .find_map(|s| {
                if let Stmt::Node { id_span, .. } = s {
                    Some(id_span.clone())
                } else {
                    None
                }
            })
            .unwrap_or(0..src.len());
        return Err(vec![CompileError {
            message: "cannot mix `participant` declarations with plain node declarations in the same diagram".to_string(),
            span,
        }]);
    }

    if has_participant {
        build_sequence_diagram(ast, src)
    } else {
        // Graph mode: dashed edges are not allowed.
        let dashed_err: Vec<CompileError> = ast
            .stmts
            .iter()
            .filter_map(|s| {
                if let Stmt::DashedEdge(e) = s {
                    Some(CompileError {
                        message: "`-->` (dashed edge) is only valid in sequence diagrams; use `->` for graph diagrams".to_string(),
                        span: e.from_span.start..e.to_span.end,
                    })
                } else {
                    None
                }
            })
            .collect();
        if !dashed_err.is_empty() {
            return Err(dashed_err);
        }
        build_graph_diagram(ast, src)
    }
}

/// Build a [`GraphDiagram`] from the AST.
fn build_graph_diagram(ast: Ast, src: &str) -> Result<Diagram, Vec<CompileError>> {
    let mut direction = Direction::Down;
    let mut graph = GraphDiagram::new(direction);
    let mut errors: Vec<CompileError> = Vec::new();

    // First pass: direction + node declarations.
    for stmt in &ast.stmts {
        match stmt {
            Stmt::Direction(d, _span) => {
                direction = *d;
            }
            Stmt::DirectionError(span) => {
                errors.push(CompileError {
                    message: "expected `down` or `right` after `direction`".to_string(),
                    span: span.clone(),
                });
            }
            Stmt::Node { id, id_span, label } => {
                // Check for duplicate node declarations.
                if graph.nodes.contains_key(id) {
                    errors.push(CompileError {
                        message: format!("duplicate node declaration `{}`", id),
                        span: id_span.clone(),
                    });
                    continue;
                }
                let label = label.clone().unwrap_or_else(|| id.clone());
                // Validate escape sequences in label: check for invalid \x.
                if let Some(err_span) = find_invalid_escape(src, &label, id_span.end) {
                    errors.push(CompileError {
                        message: "invalid escape sequence in string literal (only `\\\"` and `\\\\` are supported)".to_string(),
                        span: err_span,
                    });
                }
                graph.nodes.insert(id.clone(), Node::new(id.clone(), label));
            }
            Stmt::Edge(_) | Stmt::DashedEdge(_) | Stmt::Participant { .. } => {}
        }
    }
    graph.direction = direction;

    // Second pass: edges (validate endpoints, self-loops).
    for stmt in &ast.stmts {
        if let Stmt::Edge(e) = stmt {
            // Check for self-loops.
            if e.from == e.to {
                errors.push(CompileError {
                    message: format!(
                        "self-loops are not yet supported (edge `{}` -> `{}`)",
                        e.from, e.to
                    ),
                    span: e.from_span.start..e.to_span.end,
                });
                continue;
            }

            for (endpoint, span) in [(&e.from, &e.from_span), (&e.to, &e.to_span)] {
                if !graph.nodes.contains_key(endpoint) {
                    let mut message = format!("unknown node `{}`", endpoint);
                    if let Some(suggestion) = closest_name(endpoint, graph.nodes.keys()) {
                        message.push_str(&format!(", did you mean `{}`?", suggestion));
                    }
                    errors.push(CompileError {
                        message,
                        span: span.clone(),
                    });
                }
            }
            graph.edges.push(Edge::new(
                e.from.clone(),
                e.to.clone(),
                e.label.clone(),
                ArrowType::Triangle,
            ));
        }
    }

    if errors.is_empty() {
        Ok(Diagram::Graph(graph))
    } else {
        Err(errors)
    }
}

/// Build a [`SequenceDiagram`] from the AST.
fn build_sequence_diagram(ast: Ast, src: &str) -> Result<Diagram, Vec<CompileError>> {
    let mut seq = SequenceDiagram::new();
    let mut errors: Vec<CompileError> = Vec::new();

    // First pass: collect participants; also catch direction statements which
    // are not valid in sequence diagrams.
    for stmt in &ast.stmts {
        match stmt {
            Stmt::Direction(_, span) => {
                errors.push(CompileError {
                    message: "`direction` is not valid in sequence diagrams".to_string(),
                    span: span.clone(),
                });
                continue;
            }
            Stmt::DirectionError(span) => {
                errors.push(CompileError {
                    message: "expected `down` or `right` after `direction`".to_string(),
                    span: span.clone(),
                });
                continue;
            }
            _ => {}
        }
        if let Stmt::Participant { id, id_span, label } = stmt {
            if seq.participants.contains_key(id) {
                errors.push(CompileError {
                    message: format!("duplicate participant `{}`", id),
                    span: id_span.clone(),
                });
                continue;
            }
            let label = label.clone().unwrap_or_else(|| id.clone());
            if let Some(err_span) = find_invalid_escape(src, &label, id_span.end) {
                errors.push(CompileError {
                    message: "invalid escape sequence in string literal (only `\\\"` and `\\\\` are supported)".to_string(),
                    span: err_span,
                });
            }
            seq.participants
                .insert(id.clone(), Participant::new(id.clone(), label));
        }
    }

    // Second pass: messages (solid and dashed edges).
    for stmt in &ast.stmts {
        let (e, line_style) = match stmt {
            Stmt::Edge(e) => (e, LineStyle::Solid),
            Stmt::DashedEdge(e) => (e, LineStyle::Dashed),
            _ => continue,
        };

        // Validate that both endpoints are declared participants.
        let mut valid = true;
        for (endpoint, span) in [(&e.from, &e.from_span), (&e.to, &e.to_span)] {
            if !seq.participants.contains_key(endpoint) {
                let mut message = format!("unknown participant `{}`", endpoint);
                if let Some(suggestion) = closest_name(endpoint, seq.participants.keys()) {
                    message.push_str(&format!(", did you mean `{}`?", suggestion));
                }
                errors.push(CompileError {
                    message,
                    span: span.clone(),
                });
                valid = false;
            }
        }
        if !valid {
            continue;
        }

        // Validate escape sequence in label.
        if let Some(label) = &e.label {
            if let Some(err_span) = find_invalid_escape(src, label, e.to_span.end) {
                errors.push(CompileError {
                    message: "invalid escape sequence in string literal (only `\\\"` and `\\\\` are supported)".to_string(),
                    span: err_span,
                });
            }
        }

        seq.items.push(SequenceItem::Message(Message::new(
            e.from.clone(),
            e.to.clone(),
            e.label.clone(),
            line_style,
            ArrowType::Triangle,
        )));
    }

    if errors.is_empty() {
        Ok(Diagram::Sequence(seq))
    } else {
        Err(errors)
    }
}

/// Check if a string literal (already-parsed content) contains invalid escape
/// sequences. This is a best-effort check on the raw source.
///
/// Scans the source starting at `after` (the byte offset immediately following
/// the label owner's span), finds the first string literal `"..."` opening
/// quote, and reports the first invalid `\x` sequence inside it.
///
/// Using `after` ensures each label is checked against its own literal only,
/// avoiding duplicate diagnostics when the same source has multiple labels.
///
/// Returns the span of the first invalid `\x` sequence found, or `None`.
fn find_invalid_escape(src: &str, _label: &str, after: usize) -> Option<std::ops::Range<usize>> {
    let bytes = src.as_bytes();
    // Start scanning from `after` to locate the string literal for this label.
    let mut i = after;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            // Found the opening quote of the string literal for this label.
            i += 1;
            while i < bytes.len() && bytes[i] != b'"' {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    let next = bytes[i + 1];
                    if next != b'"' && next != b'\\' {
                        return Some(i..i + 2);
                    }
                    i += 2;
                } else {
                    i += 1;
                }
            }
            // End of this string literal — stop scanning (only check one literal).
            return None;
        }
        i += 1;
    }
    None
}

/// Find the declared name closest to `target` (Levenshtein distance <= 2).
fn closest_name<'a>(
    target: &str,
    candidates: impl Iterator<Item = &'a String>,
) -> Option<&'a String> {
    candidates
        .map(|c| (levenshtein(target, c), c))
        .filter(|(d, _)| *d <= 2)
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

/// Simple Levenshtein edit distance over Unicode scalar values.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Render compile errors to stderr as ariadne diagnostics.
pub fn report_errors(filename: &str, src: &str, errors: &[CompileError]) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_diagram() {
        let src = r#"diagram flow {
  direction down
  start: "開始"
  proc: "処理する"
  end: "終了"
  start -> proc : "次へ"
  proc -> end
}"#;
        let d = parse(src).expect("should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.direction, Direction::Down);
        assert_eq!(g.nodes.len(), 3);
        assert_eq!(g.edges.len(), 2);
        assert_eq!(g.nodes["start"].label, "開始");
        assert_eq!(g.edges[0].label.as_deref(), Some("次へ"));
    }

    #[test]
    fn node_without_label_uses_id() {
        let src = "diagram d { a\n a -> b\n b }";
        let d = parse(src).expect("should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.nodes["a"].label, "a");
    }

    #[test]
    fn undeclared_node_is_error() {
        let src = "diagram d {\n a: \"A\"\n a -> missing\n}";
        let err = parse(src).expect_err("should fail");
        assert!(err.iter().any(|e| e.message.contains("unknown node")));
    }

    #[test]
    fn undeclared_node_suggests_similar_name() {
        let src = "diagram d {\n proc: \"P\"\n start: \"S\"\n start -> prok\n}";
        let err = parse(src).expect_err("should fail");
        assert!(err.iter().any(|e| e.message.contains("unknown node `prok`")
            && e.message.contains("did you mean `proc`?")));
    }

    #[test]
    fn levenshtein_basics() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("abc", "abc"), 0);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("proc", "prok"), 1);
    }

    #[test]
    fn direction_right() {
        let src = "diagram d { direction right\n a\n b\n a -> b }";
        let d = parse(src).expect("should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.direction, Direction::Right);
    }

    #[test]
    fn direction_invalid_value_is_error() {
        let src = "diagram d { direction dwn\n a\n b }";
        let err = parse(src).expect_err("should fail on invalid direction value");
        assert!(
            err.iter()
                .any(|e| e.message.contains("expected `down` or `right`")),
            "got: {err:?}"
        );
    }

    #[test]
    fn direction_missing_value_is_error() {
        // `direction` alone at end of body (before `}`).
        let src = "diagram d { direction }";
        // This will fail at parse level or semantic level.
        let result = parse(src);
        assert!(
            result.is_err(),
            "should fail when direction value is missing"
        );
    }

    #[test]
    fn self_loop_is_error() {
        let src = "diagram d { a\n a -> a }";
        let err = parse(src).expect_err("self-loop should be an error");
        assert!(
            err.iter()
                .any(|e| e.message.contains("self-loops are not yet supported")),
            "got: {err:?}"
        );
    }

    #[test]
    fn duplicate_node_is_error() {
        let src = "diagram d { a: \"First\"\n a: \"Second\" }";
        let err = parse(src).expect_err("duplicate node should be an error");
        assert!(
            err.iter()
                .any(|e| e.message.contains("duplicate node declaration")),
            "got: {err:?}"
        );
    }

    #[test]
    fn string_escape_backslash_and_quote() {
        let src = r#"diagram d { a: "say \"hello\" and \\" }"#;
        let d = parse(src).expect("should parse escaped strings");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.nodes["a"].label, r#"say "hello" and \"#);
    }

    #[test]
    fn invalid_escape_sequence_is_error() {
        let src = r#"diagram d { a: "bad \n escape" }"#;
        let err = parse(src).expect_err("invalid escape should be an error");
        assert!(
            err.iter()
                .any(|e| e.message.contains("invalid escape sequence")),
            "got: {err:?}"
        );
    }

    // --- Sequence diagram tests ---

    #[test]
    fn parses_sequence_diagram() {
        let src = r#"diagram seq {
  participant web: "Webブラウザ"
  participant api: "APIサーバ"
  web -> api : "POST /login"
  api --> web : "200 OK"
}"#;
        let d = parse(src).expect("should parse");
        let Diagram::Sequence(s) = d else {
            panic!("expected Sequence, got {:?}", d)
        };
        assert_eq!(s.participants.len(), 2);
        assert_eq!(s.items.len(), 2);
        let kozue_ir::SequenceItem::Message(ref m0) = s.items[0] else {
            panic!()
        };
        assert_eq!(m0.line, LineStyle::Solid);
        let kozue_ir::SequenceItem::Message(ref m1) = s.items[1] else {
            panic!()
        };
        assert_eq!(m1.line, LineStyle::Dashed);
    }

    #[test]
    fn sequence_self_message_is_valid() {
        let src = r#"diagram seq {
  participant a: "Alice"
  a -> a : "think"
}"#;
        let d = parse(src).expect("self-message in sequence should be valid");
        let Diagram::Sequence(s) = d else { panic!() };
        assert_eq!(s.items.len(), 1);
    }

    // --- Issue 1: direction in sequence diagrams ---

    #[test]
    fn direction_in_sequence_diagram_is_error() {
        let src = r#"diagram seq {
  participant a: "A"
  direction down
  a -> a
}"#;
        let err = parse(src).expect_err("direction in sequence should be an error");
        assert!(
            err.iter().any(|e| e
                .message
                .contains("`direction` is not valid in sequence diagrams")),
            "got: {err:?}"
        );
    }

    #[test]
    fn direction_bogus_in_sequence_diagram_is_error() {
        let src = r#"diagram seq {
  participant a: "A"
  direction bogus
  a -> a
}"#;
        let err = parse(src).expect_err("bogus direction in sequence should be an error");
        assert!(
            err.iter()
                .any(|e| e.message.contains("expected `down` or `right`")),
            "got: {err:?}"
        );
    }

    // --- Issue 2: escape error deduplication ---

    #[test]
    fn invalid_escape_reported_once_per_label_not_multiplied() {
        // Two participants, only the second has an invalid escape.
        // The bug would report the second participant's error twice (once for each label processed).
        let src = "diagram seq {\n  participant a: \"ok\"\n  participant b: \"bad \\n escape\"\n}";
        let err = parse(src).expect_err("invalid escape should be an error");
        let escape_errors: Vec<_> = err
            .iter()
            .filter(|e| e.message.contains("invalid escape sequence"))
            .collect();
        assert_eq!(
            escape_errors.len(),
            1,
            "expected exactly 1 invalid-escape error, got {}: {err:?}",
            escape_errors.len()
        );
    }

    #[test]
    fn multiple_labels_with_independent_escapes() {
        // Each node with an invalid escape should produce exactly one error.
        let src = "diagram d { a: \"bad \\n\" b: \"also \\t bad\" a -> b }";
        let err = parse(src).expect_err("invalid escapes should be errors");
        let escape_errors: Vec<_> = err
            .iter()
            .filter(|e| e.message.contains("invalid escape sequence"))
            .collect();
        assert_eq!(
            escape_errors.len(),
            2,
            "expected exactly 2 invalid-escape errors (one per label), got {}: {err:?}",
            escape_errors.len()
        );
    }
}
