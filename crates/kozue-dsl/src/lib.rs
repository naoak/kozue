//! DSL parser for kozue using chumsky 0.9, with ariadne diagnostics.
//!
//! Grammar (M0):
//! ```text
//! diagram <name> {
//!   direction down|right
//!   <id>: "label"
//!   <a> -> <b> : "label"
//! }
//! ```

use ariadne::{Label, Report, ReportKind, Source};
use chumsky::prelude::*;
use kozue_ir::{ArrowType, Diagram, Direction, Edge, GraphDiagram, Node};

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

    // Edge: `a -> b` optionally `: "label"`.
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
    // then edge (has `->`) before node.
    let stmt = direction.or(edge).or(node);

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

/// Semantic pass: assemble the [`GraphDiagram`] and validate references.
fn build_diagram(ast: Ast, src: &str) -> Result<Diagram, Vec<CompileError>> {
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
            Stmt::Edge(_) => {}
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

/// Check if a string literal (already-parsed content) contains invalid escape
/// sequences. This is a best-effort check on the raw source.
/// Returns the span of the first invalid `\x` sequence found, or `None`.
fn find_invalid_escape(src: &str, _label: &str, _after: usize) -> Option<std::ops::Range<usize>> {
    // We scan the raw source for string literals that contain `\` followed by
    // a character that is not `"` or `\`. We do this by looking for `"` delimiters.
    // This is best-effort: we look within the full source for any such pattern.
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            // Found a string literal start.
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
            i += 1; // skip closing `"`
        } else {
            i += 1;
        }
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
}
