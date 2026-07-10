//! DSL parser for kozue using chumsky 0.9, with ariadne diagnostics.
//!
//! Grammar (M0 + M2a):
//! ```text
//! diagram <name> {
//!   // line comments are allowed anywhere
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
        /// Span of the string literal (including quotes), if present.
        label_lit_span: Option<std::ops::Range<usize>>,
    },
    Edge(EdgeStmt),
    DashedEdge(EdgeStmt),
    Participant {
        id: String,
        id_span: std::ops::Range<usize>,
        label: Option<String>,
        /// Span of the string literal (including quotes), if present.
        label_lit_span: Option<std::ops::Range<usize>>,
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
    /// Span of the label string literal (including quotes), if present.
    label_lit_span: Option<std::ops::Range<usize>>,
}

#[derive(Debug, Clone)]
struct Ast {
    name: String,
    name_span: std::ops::Range<usize>,
    stmts: Vec<Stmt>,
}

/// A user-facing error with a source span, for pretty diagnostics.
#[derive(Debug, Clone)]
pub struct CompileError {
    pub message: String,
    pub span: std::ops::Range<usize>,
    /// Optional secondary label: an extra source location with its own message,
    /// rendered as a second ariadne label (e.g. "first declared here" for
    /// duplicate declaration errors).
    pub secondary: Option<(std::ops::Range<usize>, String)>,
}

// ---------------------------------------------------------------------------
// Comment-aware padding
// ---------------------------------------------------------------------------

/// A `//` line comment: consumes `//` and everything up to (but not including)
/// the next newline or end of input.
fn line_comment() -> impl Parser<char, (), Error = Simple<char>> + Clone {
    just("//")
        .then(filter(|c: &char| *c != '\n').repeated())
        .ignored()
}

/// Padding that treats both ASCII whitespace and `//` line comments as noise.
fn kzd_ws() -> impl Parser<char, (), Error = Simple<char>> + Clone {
    line_comment()
        .or(filter(|c: &char| c.is_whitespace()).ignored())
        .repeated()
        .ignored()
}

// ---------------------------------------------------------------------------
// Token-level helpers
// ---------------------------------------------------------------------------

fn ident_spanned(
) -> impl Parser<char, (String, std::ops::Range<usize>), Error = Simple<char>> + Clone {
    // Apply map_with_span to text::ident() BEFORE consuming surrounding whitespace,
    // so the span covers only the identifier characters themselves.
    text::ident()
        .map_with_span(|s, span| (s, span))
        .padded_by(kzd_ws())
}

/// Parse a string literal with escape sequences: `\"` and `\\`.
/// Returns `(content, literal_span)` where `literal_span` covers the entire
/// `"..."` token (including the surrounding quotes).
fn string_lit_spanned(
) -> impl Parser<char, (String, std::ops::Range<usize>), Error = Simple<char>> + Clone {
    let char_inner = just('\\')
        .ignore_then(
            just('"')
                .to('"')
                .or(just('\\').to('\\'))
                .or(none_of("\"").map(|c: char| c)),
        )
        .or(none_of("\"\\"));

    just('"')
        .ignore_then(char_inner.repeated())
        .then_ignore(just('"'))
        .collect::<String>()
        .map_with_span(|s, span| (s, span))
        .padded_by(kzd_ws())
}

fn parser() -> impl Parser<char, Ast, Error = Simple<char>> {
    // direction statement: `direction down|right`
    let direction_kw = text::keyword("direction").padded_by(kzd_ws());
    let direction_val = text::keyword("down")
        .to(Direction::Down)
        .or(text::keyword("right").to(Direction::Right));

    let direction = direction_kw
        .ignore_then(
            direction_val
                .map_with_span(Stmt::Direction)
                .or(text::ident()
                    .padded_by(kzd_ws())
                    .map_with_span(|_, span| Stmt::DirectionError(span)))
                .or(empty().map_with_span(|_, span| Stmt::DirectionError(span))),
        )
        .map_with_span(|s, _span| s);

    // participant: `participant id` or `participant id: "label"`
    let participant = text::keyword("participant")
        .padded_by(kzd_ws())
        .ignore_then(ident_spanned())
        .then(
            just(':')
                .padded_by(kzd_ws())
                .ignore_then(string_lit_spanned())
                .or_not(),
        )
        .map(|((id, id_span), label_opt)| {
            let (label, label_lit_span) = match label_opt {
                Some((l, s)) => (Some(l), Some(s)),
                None => (None, None),
            };
            Stmt::Participant {
                id,
                id_span,
                label,
                label_lit_span,
            }
        });

    // Dashed edge: `a --> b` optionally `: "label"`
    let dashed_edge = ident_spanned()
        .then_ignore(just("-->").padded_by(kzd_ws()))
        .then(ident_spanned())
        .then(
            just(':')
                .padded_by(kzd_ws())
                .ignore_then(string_lit_spanned())
                .or_not(),
        )
        .map(|(((from, from_span), (to, to_span)), label_opt)| {
            let (label, label_lit_span) = match label_opt {
                Some((l, s)) => (Some(l), Some(s)),
                None => (None, None),
            };
            Stmt::DashedEdge(EdgeStmt {
                from,
                from_span,
                to,
                to_span,
                label,
                label_lit_span,
            })
        });

    // Solid edge: `a -> b` optionally `: "label"`.
    let edge = ident_spanned()
        .then_ignore(just("->").padded_by(kzd_ws()))
        .then(ident_spanned())
        .then(
            just(':')
                .padded_by(kzd_ws())
                .ignore_then(string_lit_spanned())
                .or_not(),
        )
        .map(|(((from, from_span), (to, to_span)), label_opt)| {
            let (label, label_lit_span) = match label_opt {
                Some((l, s)) => (Some(l), Some(s)),
                None => (None, None),
            };
            Stmt::Edge(EdgeStmt {
                from,
                from_span,
                to,
                to_span,
                label,
                label_lit_span,
            })
        });

    // Node: `id : "label"` or `id`.
    let node = ident_spanned()
        .then(
            just(':')
                .padded_by(kzd_ws())
                .ignore_then(string_lit_spanned())
                .or_not(),
        )
        .map(|((id, id_span), label_opt)| {
            let (label, label_lit_span) = match label_opt {
                Some((l, s)) => (Some(l), Some(s)),
                None => (None, None),
            };
            Stmt::Node {
                id,
                id_span,
                label,
                label_lit_span,
            }
        });

    let stmt = direction.or(participant).or(dashed_edge).or(edge).or(node);
    let body = stmt.repeated().padded_by(kzd_ws());

    text::keyword("diagram")
        .padded_by(kzd_ws())
        .ignore_then(
            text::ident()
                .padded_by(kzd_ws())
                .map_with_span(|s, span| (s, span)),
        )
        .then_ignore(just('{').padded_by(kzd_ws()))
        .then(body)
        .then_ignore(just('}').padded_by(kzd_ws()))
        .then_ignore(end())
        .map(|((name, name_span), stmts)| Ast {
            name,
            name_span,
            stmts,
        })
}

/// Parse source text into a semantic [`Diagram`], collecting errors.
pub fn parse(src: &str) -> Result<Diagram, Vec<CompileError>> {
    let ast = parser().parse(src).map_err(|errs| {
        errs.into_iter()
            .map(|e| CompileError {
                message: format!("{}", e),
                span: e.span(),
                secondary: None,
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
            secondary: None,
        }]);
    }

    if has_participant {
        build_sequence_diagram(ast, src)
    } else {
        let dashed_err: Vec<CompileError> = ast
            .stmts
            .iter()
            .filter_map(|s| {
                if let Stmt::DashedEdge(e) = s {
                    Some(CompileError {
                        message: "`-->` (dashed edge) is only valid in sequence diagrams; use `->` for graph diagrams".to_string(),
                        span: e.from_span.start..e.to_span.end,
                        secondary: None,
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
    // First-declaration spans, for "first declared here" secondary labels.
    let mut first_decl_spans: std::collections::BTreeMap<String, std::ops::Range<usize>> =
        std::collections::BTreeMap::new();

    for stmt in &ast.stmts {
        match stmt {
            Stmt::Direction(d, _span) => {
                direction = *d;
            }
            Stmt::DirectionError(span) => {
                errors.push(CompileError {
                    message: "expected `down` or `right` after `direction`".to_string(),
                    span: span.clone(),
                    secondary: None,
                });
            }
            Stmt::Node {
                id,
                id_span,
                label,
                label_lit_span,
            } => {
                if graph.nodes.contains_key(id) {
                    errors.push(CompileError {
                        message: format!("duplicate node declaration `{}`", id),
                        span: id_span.clone(),
                        secondary: first_decl_spans
                            .get(id)
                            .map(|s| (s.clone(), "first declared here".to_string())),
                    });
                    continue;
                }
                let label_str = label.clone().unwrap_or_else(|| id.clone());
                if let Some(lit_span) = label_lit_span {
                    if let Some(err_span) = find_invalid_escape_in_span(src, lit_span) {
                        errors.push(CompileError {
                            message: "invalid escape sequence in string literal (only `\\\"` and `\\\\` are supported)".to_string(),
                            span: err_span,
                            secondary: None,
                        });
                    }
                }
                first_decl_spans.insert(id.clone(), id_span.clone());
                graph
                    .nodes
                    .insert(id.clone(), Node::new(id.clone(), label_str));
            }
            Stmt::Edge(_) | Stmt::DashedEdge(_) | Stmt::Participant { .. } => {}
        }
    }
    graph.direction = direction;

    for stmt in &ast.stmts {
        if let Stmt::Edge(e) = stmt {
            if e.from == e.to {
                errors.push(CompileError {
                    message: format!(
                        "self-loops are not yet supported (edge `{}` -> `{}`)",
                        e.from, e.to
                    ),
                    span: e.from_span.start..e.to_span.end,
                    secondary: None,
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
                        secondary: None,
                    });
                }
            }
            if let Some(label_lit_span) = &e.label_lit_span {
                if let Some(err_span) = find_invalid_escape_in_span(src, label_lit_span) {
                    errors.push(CompileError {
                        message: "invalid escape sequence in string literal (only `\\\"` and `\\\\` are supported)".to_string(),
                        span: err_span,
                        secondary: None,
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
    // First-declaration spans, for "first declared here" secondary labels.
    let mut first_decl_spans: std::collections::BTreeMap<String, std::ops::Range<usize>> =
        std::collections::BTreeMap::new();

    for stmt in &ast.stmts {
        match stmt {
            Stmt::Direction(_, span) => {
                errors.push(CompileError {
                    message: "`direction` is not valid in sequence diagrams".to_string(),
                    span: span.clone(),
                    secondary: None,
                });
                continue;
            }
            Stmt::DirectionError(span) => {
                errors.push(CompileError {
                    message: "expected `down` or `right` after `direction`".to_string(),
                    span: span.clone(),
                    secondary: None,
                });
                continue;
            }
            _ => {}
        }
        if let Stmt::Participant {
            id,
            id_span,
            label,
            label_lit_span,
        } = stmt
        {
            if seq.participants.contains_key(id) {
                errors.push(CompileError {
                    message: format!("duplicate participant `{}`", id),
                    span: id_span.clone(),
                    secondary: first_decl_spans
                        .get(id)
                        .map(|s| (s.clone(), "first declared here".to_string())),
                });
                continue;
            }
            let label_str = label.clone().unwrap_or_else(|| id.clone());
            if let Some(lit_span) = label_lit_span {
                if let Some(err_span) = find_invalid_escape_in_span(src, lit_span) {
                    errors.push(CompileError {
                        message: "invalid escape sequence in string literal (only `\\\"` and `\\\\` are supported)".to_string(),
                        span: err_span,
                        secondary: None,
                    });
                }
            }
            first_decl_spans.insert(id.clone(), id_span.clone());
            seq.participants
                .insert(id.clone(), Participant::new(id.clone(), label_str));
        }
    }

    for stmt in &ast.stmts {
        let (e, line_style) = match stmt {
            Stmt::Edge(e) => (e, LineStyle::Solid),
            Stmt::DashedEdge(e) => (e, LineStyle::Dashed),
            _ => continue,
        };

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
                    secondary: None,
                });
                valid = false;
            }
        }
        if !valid {
            continue;
        }

        if let Some(label_lit_span) = &e.label_lit_span {
            if let Some(err_span) = find_invalid_escape_in_span(src, label_lit_span) {
                errors.push(CompileError {
                    message: "invalid escape sequence in string literal (only `\\\"` and `\\\\` are supported)".to_string(),
                    span: err_span,
                    secondary: None,
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

/// Check for invalid escape sequences inside the exact span of a string literal.
///
/// `lit_span` is a **character-index** range (as returned by chumsky 0.9's
/// `map_with_span`) covering the entire `"..."` token including quotes.
/// We convert to byte offsets for scanning and return a byte-offset span of
/// the first invalid `\x` sequence, or `None`.
fn find_invalid_escape_in_span(
    src: &str,
    lit_span: &std::ops::Range<usize>,
) -> Option<std::ops::Range<usize>> {
    // Convert char-index span boundaries to byte offsets.
    // lit_span.start is the `"` opening quote; skip it (+1 char).
    let byte_start = char_idx_to_byte_offset(src, lit_span.start + 1);
    let byte_end = char_idx_to_byte_offset(src, lit_span.end);
    let bytes = src.as_bytes();
    let end = byte_end.min(bytes.len());
    let mut i = byte_start;
    while i < end {
        if bytes[i] == b'"' {
            // Closing quote — end of content.
            break;
        }
        if bytes[i] == b'\\' && i + 1 < end {
            let next = bytes[i + 1];
            if next != b'"' && next != b'\\' {
                return Some(i..i + 2);
            }
            i += 2;
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
        let mut report = Report::build(ReportKind::Error, filename, span.start)
            .with_message(&err.message)
            .with_label(Label::new((filename, span)).with_message(&err.message));
        if let Some((sec_span, sec_msg)) = &err.secondary {
            report = report.with_label(
                Label::new((filename, sec_span.clone()))
                    .with_message(sec_msg)
                    .with_order(1),
            );
        }
        report.finish().eprint((filename, Source::from(src))).ok();
    }
}

// ---------------------------------------------------------------------------
// Formatter (M3a Part 3)
// ---------------------------------------------------------------------------

/// A raw comment extracted from source text.
#[derive(Debug, Clone)]
struct RawComment {
    /// 0-indexed line number in the source.
    line: usize,
    /// Full comment text including `//`.
    text: String,
    /// True if there is non-whitespace before `//` on this line (trailing comment).
    is_trailing: bool,
}

/// Extract all `//` comments from source, respecting string literal boundaries.
fn extract_comments(src: &str) -> Vec<RawComment> {
    let mut comments = Vec::new();
    let bytes = src.as_bytes();
    let mut i = 0;
    let mut line = 0usize;

    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                // Skip string literal content.
                i += 1;
                while i < bytes.len() {
                    match bytes[i] {
                        b'"' => {
                            i += 1;
                            break;
                        }
                        b'\\' => i += 2, // skip escape pair
                        b'\n' => {
                            line += 1;
                            i += 1;
                        }
                        _ => i += 1,
                    }
                }
            }
            b'\n' => {
                line += 1;
                i += 1;
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                // Found a comment. Check if it's trailing.
                let line_start = src[..i].rfind('\n').map(|p| p + 1).unwrap_or(0);
                let before = &src[line_start..i];
                let is_trailing = before.chars().any(|c| !c.is_whitespace());

                // Collect to end of line.
                let comment_start = i;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                let text = src[comment_start..i].trim_end().to_string();
                comments.push(RawComment {
                    line,
                    text,
                    is_trailing,
                });
            }
            _ => {
                i += 1;
            }
        }
    }
    comments
}

/// Information about a statement's position in source for comment association.
#[derive(Debug, Clone)]
struct StmtPos {
    /// 0-indexed line number where this statement starts.
    start_line: usize,
    /// 0-indexed line number where this statement ends.
    end_line: usize,
}

/// Compute the line number (0-indexed) for a **character** index in source.
///
/// Chumsky 0.9 uses character indices (not byte offsets) for spans when parsing
/// `&str`. We count `\n` characters up to (but not including) `char_idx`.
fn char_idx_to_line(src: &str, char_idx: usize) -> usize {
    src.chars().take(char_idx).filter(|&c| c == '\n').count()
}

/// Convert a character index (as used by chumsky 0.9 spans) to a byte offset in `src`.
fn char_idx_to_byte_offset(src: &str, char_idx: usize) -> usize {
    src.char_indices()
        .nth(char_idx)
        .map(|(byte_off, _)| byte_off)
        .unwrap_or(src.len())
}

/// Format a string value back to a DSL string literal, re-escaping as needed.
fn format_string_lit(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// Formatted lines with optional trailing comment.
#[derive(Debug, Clone)]
struct FormattedLine {
    /// The formatted code (without trailing comment).
    code: String,
    /// Optional trailing comment (from source).
    trailing_comment: Option<String>,
}

impl FormattedLine {
    fn new(code: impl Into<String>) -> Self {
        FormattedLine {
            code: code.into(),
            trailing_comment: None,
        }
    }

    fn render(&self) -> String {
        match &self.trailing_comment {
            Some(c) => format!("{}  {}", self.code, c),
            None => self.code.clone(),
        }
    }
}

/// Format the kozue DSL source into its canonical normal form.
///
/// Returns the formatted string, or errors if the source fails to parse.
pub fn format_kzd(src: &str) -> Result<String, Vec<CompileError>> {
    // Parse to get the AST with spans.
    let ast = parser().parse(src).map_err(|errs| {
        errs.into_iter()
            .map(|e| CompileError {
                message: format!("{}", e),
                span: e.span(),
                secondary: None,
            })
            .collect::<Vec<_>>()
    })?;

    // Also run semantic validation to surface semantic errors.
    build_diagram(ast.clone(), src)?;

    // Extract comments from source.
    let comments = extract_comments(src);

    // Compute the line of the `diagram` keyword.
    // name_span points to the diagram name; the `diagram` keyword itself is just
    // before it. For comment categorization we only need a rough line.
    let diagram_kw_line = char_idx_to_line(src, ast.name_span.start);

    // Compute per-statement source positions.
    let stmt_positions: Vec<StmtPos> = ast
        .stmts
        .iter()
        .map(|stmt| {
            let (start_off, end_off) = stmt_span(stmt);
            StmtPos {
                start_line: char_idx_to_line(src, start_off),
                end_line: char_idx_to_line(src, end_off),
            }
        })
        .collect();

    // --- Comment association ---
    //
    // We split comments into:
    //   header_comments  : standalone comments strictly before the `diagram` keyword line
    //   stmt_trailing[i] : the comment that trails statement i on the same line
    //   stmt_leading[i]  : standalone comments between the previous stmt and stmt i
    //                      (including those between `{` and the first stmt)
    //   trailing_body    : standalone comments after the last statement (before `}`)
    //
    // Comments are mutually exclusive — each is counted in exactly one bucket.

    // Header comments: standalone, before `diagram` keyword.
    let header_comments: Vec<String> = comments
        .iter()
        .filter(|c| !c.is_trailing && c.line < diagram_kw_line)
        .map(|c| c.text.clone())
        .collect();

    let mut stmt_trailing: Vec<Option<String>> = vec![None; ast.stmts.len()];
    let mut stmt_leading: Vec<Vec<String>> = vec![Vec::new(); ast.stmts.len()];
    let mut trailing_body_comments: Vec<String> = Vec::new();

    for comment in &comments {
        // Skip header comments (already collected).
        if !comment.is_trailing && comment.line < diagram_kw_line {
            continue;
        }
        if comment.is_trailing {
            // Find the statement on this line.
            if let Some(idx) = stmt_positions
                .iter()
                .position(|p| p.end_line == comment.line)
            {
                stmt_trailing[idx] = Some(comment.text.clone());
            } else if comment.line == diagram_kw_line && !ast.stmts.is_empty() {
                // Comment trailing the `diagram ... {` line: attach as leading
                // comment of the first statement in the body.
                stmt_leading[0].push(comment.text.clone());
            }
            // (Other trailing comments that don't match any statement are discarded.)
        } else {
            // Standalone: attach as leading comment of the first statement
            // that starts after this comment's line.
            if let Some(idx) = stmt_positions
                .iter()
                .position(|p| p.start_line > comment.line)
            {
                stmt_leading[idx].push(comment.text.clone());
            } else {
                // After the last statement — emit before `}`.
                trailing_body_comments.push(comment.text.clone());
            }
        }
    }

    // Split statements into their rendering categories.
    let direction_idx = ast
        .stmts
        .iter()
        .position(|s| matches!(s, Stmt::Direction(..)));
    let direction_error_indices: Vec<usize> = ast
        .stmts
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            if matches!(s, Stmt::DirectionError(_)) {
                Some(i)
            } else {
                None
            }
        })
        .collect();

    let decl_indices: Vec<usize> = ast
        .stmts
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            if matches!(s, Stmt::Node { .. } | Stmt::Participant { .. }) {
                Some(i)
            } else {
                None
            }
        })
        .collect();

    let edge_indices: Vec<usize> = ast
        .stmts
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            if matches!(s, Stmt::Edge(_) | Stmt::DashedEdge(_)) {
                Some(i)
            } else {
                None
            }
        })
        .collect();

    let mut out = String::new();

    // Header comments (before `diagram`).
    for c in &header_comments {
        out.push_str(c);
        out.push('\n');
    }

    // `diagram <name> {`
    out.push_str(&format!("diagram {} {{", ast.name));
    out.push('\n');

    // Direction statement (and its leading standalone comments).
    let has_direction = direction_idx.is_some() || !direction_error_indices.is_empty();
    if let Some(idx) = direction_idx {
        let dir_str = match &ast.stmts[idx] {
            Stmt::Direction(Direction::Down, _) => "direction down",
            Stmt::Direction(Direction::Right, _) => "direction right",
            _ => unreachable!(),
        };
        for lc in &stmt_leading[idx] {
            out.push_str("  ");
            out.push_str(lc);
            out.push('\n');
        }
        let mut fl = FormattedLine::new(format!("  {}", dir_str));
        fl.trailing_comment = stmt_trailing[idx].clone();
        out.push_str(&fl.render());
        out.push('\n');
    }
    // Direction errors: we've already rejected them via build_diagram above,
    // so this branch is unreachable in practice.
    for &idx in &direction_error_indices {
        let _ = idx;
    }

    // Blank line after direction (if direction present and there are decls or edges).
    if has_direction && (!decl_indices.is_empty() || !edge_indices.is_empty()) {
        out.push('\n');
    }

    // Declaration statements (with their leading standalone comments).
    for &idx in &decl_indices {
        for lc in &stmt_leading[idx] {
            out.push_str("  ");
            out.push_str(lc);
            out.push('\n');
        }
        let code = format_decl_stmt(&ast.stmts[idx]);
        let mut fl = FormattedLine::new(format!("  {}", code));
        fl.trailing_comment = stmt_trailing[idx].clone();
        out.push_str(&fl.render());
        out.push('\n');
    }

    // Blank line between decls and edges.
    if !decl_indices.is_empty() && !edge_indices.is_empty() {
        out.push('\n');
    }

    // Edge/message statements (with their leading standalone comments).
    for &idx in &edge_indices {
        for lc in &stmt_leading[idx] {
            out.push_str("  ");
            out.push_str(lc);
            out.push('\n');
        }
        let code = format_edge_stmt(&ast.stmts[idx]);
        let mut fl = FormattedLine::new(format!("  {}", code));
        fl.trailing_comment = stmt_trailing[idx].clone();
        out.push_str(&fl.render());
        out.push('\n');
    }

    // Trailing body comments (standalone comments after last statement).
    for c in &trailing_body_comments {
        out.push_str("  ");
        out.push_str(c);
        out.push('\n');
    }

    out.push_str("}\n");

    Ok(out)
}

/// Get the (start, end) byte span of a statement.
fn stmt_span(stmt: &Stmt) -> (usize, usize) {
    match stmt {
        Stmt::Direction(_, span) | Stmt::DirectionError(span) => (span.start, span.end),
        Stmt::Node {
            id_span,
            label_lit_span,
            ..
        }
        | Stmt::Participant {
            id_span,
            label_lit_span,
            ..
        } => {
            let end = label_lit_span
                .as_ref()
                .map(|s| s.end)
                .unwrap_or(id_span.end);
            (id_span.start, end)
        }
        Stmt::Edge(e) | Stmt::DashedEdge(e) => {
            let end = e
                .label_lit_span
                .as_ref()
                .map(|s| s.end)
                .unwrap_or(e.to_span.end);
            (e.from_span.start, end)
        }
    }
}

/// Format a declaration statement (Node or Participant).
fn format_decl_stmt(stmt: &Stmt) -> String {
    match stmt {
        Stmt::Node { id, label, .. } => {
            if let Some(label_str) = label {
                format!("{}: {}", id, format_string_lit(label_str))
            } else {
                id.clone()
            }
        }
        Stmt::Participant { id, label, .. } => {
            if let Some(label_str) = label {
                format!("participant {}: {}", id, format_string_lit(label_str))
            } else {
                format!("participant {}", id)
            }
        }
        _ => String::new(),
    }
}

/// Format an edge statement (Edge or DashedEdge).
fn format_edge_stmt(stmt: &Stmt) -> String {
    match stmt {
        Stmt::Edge(e) => {
            if let Some(label_str) = &e.label {
                format!("{} -> {} : {}", e.from, e.to, format_string_lit(label_str))
            } else {
                format!("{} -> {}", e.from, e.to)
            }
        }
        Stmt::DashedEdge(e) => {
            if let Some(label_str) = &e.label {
                format!("{} --> {} : {}", e.from, e.to, format_string_lit(label_str))
            } else {
                format!("{} --> {}", e.from, e.to)
            }
        }
        _ => String::new(),
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
        let src = "diagram d { direction }";
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

    // --- M3a Part 1: Span precision tests ---

    #[test]
    fn duplicate_node_span_points_to_second_occurrence() {
        // `a` appears at offsets 13 and 26 (approximately).
        // The error span should point to the second `a`, not the first.
        let src = "diagram d { a: \"First\"\n a: \"Second\" }";
        let err = parse(src).expect_err("duplicate node should be an error");
        let dup_err = err
            .iter()
            .find(|e| e.message.contains("duplicate node declaration"))
            .expect("should have duplicate error");
        // The second `a` starts after the newline at position 23.
        // In the source "diagram d { a: \"First\"\n a: \"Second\" }"
        //                0123456789012345678901234567890
        // Position of first `a`: 12
        // Position of second `a`: 24 (after \n and space)
        assert!(
            dup_err.span.start > 12,
            "duplicate error span should point to second occurrence, span={:?}",
            dup_err.span
        );
        // Secondary label must point to the first declaration.
        let (sec_span, sec_msg) = dup_err
            .secondary
            .as_ref()
            .expect("duplicate error should carry a secondary label");
        assert_eq!(sec_msg, "first declared here");
        assert_eq!(
            &src[sec_span.clone()],
            "a",
            "secondary span should cover the first `a`"
        );
        assert!(
            sec_span.start < dup_err.span.start,
            "secondary span must precede the primary span"
        );
    }

    #[test]
    fn duplicate_participant_span_points_to_second_occurrence() {
        let src = "diagram seq {\n  participant a: \"A\"\n  participant a: \"B\"\n}";
        let err = parse(src).expect_err("duplicate participant should be an error");
        let dup_err = err
            .iter()
            .find(|e| e.message.contains("duplicate participant"))
            .expect("should have duplicate error");
        // First `a` appears around offset 25, second around offset 48.
        assert!(
            dup_err.span.start > 25,
            "duplicate error span should point to second occurrence, span={:?}",
            dup_err.span
        );
        // Secondary label must point to the first declaration.
        let (sec_span, sec_msg) = dup_err
            .secondary
            .as_ref()
            .expect("duplicate error should carry a secondary label");
        assert_eq!(sec_msg, "first declared here");
        assert_eq!(
            &src[sec_span.clone()],
            "a",
            "secondary span should cover the first `a`"
        );
        assert!(
            sec_span.start < dup_err.span.start,
            "secondary span must precede the primary span"
        );
    }

    #[test]
    fn unknown_node_span_exact() {
        // `ghost` appears only once; the error span must cover it precisely.
        let src = "diagram d {\n a: \"A\"\n a -> ghost\n}";
        let err = parse(src).expect_err("should fail");
        let unk_err = err
            .iter()
            .find(|e| e.message.contains("unknown node"))
            .expect("should have unknown-node error");
        let span_text = &src[unk_err.span.clone()];
        assert_eq!(
            span_text, "ghost",
            "error span should cover exactly `ghost`"
        );
    }

    #[test]
    fn invalid_escape_span_exact_second_occurrence() {
        // Both `a` and `b` labels contain identically-named chars but only
        // the second has an invalid escape. The error span must point into
        // the second literal, not the first.
        let src = "diagram d { a: \"ok\" b: \"bad \\n escape\" a -> b }";
        let err = parse(src).expect_err("invalid escape should be an error");
        let esc_err = err
            .iter()
            .find(|e| e.message.contains("invalid escape sequence"))
            .expect("should have escape error");
        // `\n` in the second literal starts after position 24 (b: "bad ...)
        // First literal ends around position 18. Error must be after that.
        assert!(
            esc_err.span.start > 18,
            "escape error span should be in the second literal, span={:?}",
            esc_err.span
        );
        // The span should cover `\n` (2 bytes).
        let span_text = &src[esc_err.span.clone()];
        assert_eq!(
            span_text, "\\n",
            "error span should cover `\\n`, got {:?}",
            span_text
        );
    }

    // --- M3a Part 2: Line comment tests ---

    #[test]
    fn line_comment_at_top_level() {
        let src = "// a comment\ndiagram d { a\n b\n a -> b }";
        let d = parse(src).expect("comment before diagram should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.nodes.len(), 2);
    }

    #[test]
    fn line_comment_inside_body() {
        let src = "diagram d {\n  // standalone comment\n  a: \"A\"\n  b: \"B\"\n  a -> b\n}";
        let d = parse(src).expect("comment inside body should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.nodes.len(), 2);
    }

    #[test]
    fn trailing_comment_after_statement() {
        let src = "diagram d {\n  a: \"A\"  // node A\n  b: \"B\"\n  a -> b\n}";
        let d = parse(src).expect("trailing comment should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.nodes["a"].label, "A");
    }

    #[test]
    fn double_slash_inside_string_is_not_comment() {
        // `//` inside a string literal should not start a comment.
        let src = r#"diagram d { a: "http://example.com" b: "B" a -> b }"#;
        let d = parse(src).expect("// inside string should not be a comment");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.nodes["a"].label, "http://example.com");
    }

    #[test]
    fn comment_does_not_affect_golden_parse() {
        // Source identical to chain.kzd but with added comments should produce
        // the same IR as the original.
        let src_no_comment = "diagram chain {\n  direction down\n\n  a: \"入力\"\n  b: \"変換\"\n  c: \"出力\"\n\n  a -> b : \"read\"\n  b -> c : \"write\"\n}\n";
        let src_with_comment = "// Chain diagram\ndiagram chain {\n  direction down  // layout direction\n\n  a: \"入力\"\n  b: \"変換\"\n  c: \"出力\"\n\n  a -> b : \"read\"\n  b -> c : \"write\"\n}\n";
        let d1 = parse(src_no_comment).expect("no-comment should parse");
        let d2 = parse(src_with_comment).expect("with-comment should parse");
        assert_eq!(d1, d2, "comments should not affect the parsed IR");
    }

    // --- M3a Part 3: Formatter tests ---

    #[test]
    fn fmt_simple_graph_is_canonical() {
        let src = "diagram d{a:\"A\"\nb:\"B\"\na->b}";
        let formatted = format_kzd(src).expect("should format");
        // Must be parseable.
        parse(&formatted).expect("formatted output should parse");
        // Must be idempotent.
        let formatted2 = format_kzd(&formatted).expect("second format should succeed");
        assert_eq!(formatted, formatted2, "fmt must be idempotent");
    }

    #[test]
    fn fmt_idempotent_on_golden_chain() {
        let src = "diagram chain {\n  direction down\n\n  a: \"入力\"\n  b: \"変換\"\n  c: \"出力\"\n\n  a -> b : \"read\"\n  b -> c : \"write\"\n}\n";
        let formatted = format_kzd(src).expect("should format");
        let formatted2 = format_kzd(&formatted).expect("second format should succeed");
        assert_eq!(formatted, formatted2, "fmt must be idempotent");
    }

    #[test]
    fn fmt_semantic_preservation() {
        let src = "diagram chain {\n  direction down\n\n  a: \"入力\"\n  b: \"変換\"\n  c: \"出力\"\n\n  a -> b : \"read\"\n  b -> c : \"write\"\n}\n";
        let formatted = format_kzd(src).expect("should format");
        let d1 = parse(src).expect("original should parse");
        let d2 = parse(&formatted).expect("formatted should parse");
        assert_eq!(d1, d2, "fmt must preserve semantics");
    }

    #[test]
    fn fmt_syntax_error_returns_error() {
        let src = "diagram d { bad syntax !!! }";
        let result = format_kzd(src);
        assert!(result.is_err(), "fmt on invalid source should return error");
    }

    #[test]
    fn fmt_preserves_trailing_comment() {
        let src = "diagram d {\n  a: \"A\"  // node a\n  b: \"B\"\n  a -> b\n}";
        let formatted = format_kzd(src).expect("should format");
        assert!(
            formatted.contains("// node a"),
            "trailing comment should be preserved: {formatted}"
        );
        // Idempotent.
        let formatted2 = format_kzd(&formatted).expect("second format");
        assert_eq!(
            formatted, formatted2,
            "fmt must be idempotent with comments"
        );
    }

    #[test]
    fn fmt_preserves_standalone_comment() {
        let src = "diagram d {\n  // declarations\n  a: \"A\"\n  b: \"B\"\n  a -> b\n}";
        let formatted = format_kzd(src).expect("should format");
        assert!(
            formatted.contains("// declarations"),
            "standalone comment should be preserved: {formatted}"
        );
        let formatted2 = format_kzd(&formatted).expect("second format");
        assert_eq!(
            formatted, formatted2,
            "fmt must be idempotent with comments"
        );
    }

    #[test]
    fn fmt_sequence_diagram() {
        let src = "diagram seq {\n  participant a: \"Alice\"\n  participant b: \"Bob\"\n  a -> b : \"hello\"\n  b --> a : \"reply\"\n}\n";
        let formatted = format_kzd(src).expect("should format");
        let d1 = parse(src).expect("original should parse");
        let d2 = parse(&formatted).expect("formatted should parse");
        assert_eq!(d1, d2, "fmt must preserve sequence diagram semantics");
        let formatted2 = format_kzd(&formatted).expect("second format");
        assert_eq!(formatted, formatted2, "fmt must be idempotent");
    }

    #[test]
    fn fmt_direction_right() {
        let src = "diagram p {\n  direction right\n  src: \"S\"\n  dst: \"D\"\n  src -> dst\n}\n";
        let formatted = format_kzd(src).expect("should format");
        assert!(
            formatted.contains("direction right"),
            "direction must be present"
        );
        let formatted2 = format_kzd(&formatted).expect("second format");
        assert_eq!(formatted, formatted2, "fmt must be idempotent");
    }

    #[test]
    fn fmt_comment_before_edge_section() {
        // Standalone comment before the first edge must appear before that edge.
        let src = "diagram d {\n  // nodes section\n  a: \"A\"\n  b: \"B\"\n  // edges section\n  a -> b\n}";
        let formatted = format_kzd(src).expect("should format");
        // `// edges section` must appear before `a -> b`.
        let edges_pos = formatted
            .find("// edges section")
            .expect("comment must be preserved");
        let edge_pos = formatted.find("a -> b").expect("edge must be present");
        assert!(
            edges_pos < edge_pos,
            "edge comment must appear before the edge: {formatted}"
        );
        // Idempotent.
        let formatted2 = format_kzd(&formatted).expect("second format");
        assert_eq!(formatted, formatted2, "fmt must be idempotent");
    }

    #[test]
    fn fmt_idempotent_on_golden_chain_with_comments() {
        // Read the actual golden chain.kzd which now has comments.
        let src = include_str!("../../../tests/golden/chain.kzd");
        let formatted = format_kzd(src).expect("should format");
        let formatted2 = format_kzd(&formatted).expect("second format");
        assert_eq!(
            formatted, formatted2,
            "fmt must be idempotent on commented chain.kzd"
        );
        // Parse result must match original.
        let d1 = parse(src).expect("original should parse");
        let d2 = parse(&formatted).expect("formatted should parse");
        assert_eq!(d1, d2, "fmt must preserve semantics");
    }

    #[test]
    fn fmt_idempotent_and_semantic_preserving_on_all_goldens() {
        // Iterate over every tests/golden/*.kzd in the workspace.
        let mut golden_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        golden_dir.pop(); // crates
        golden_dir.pop(); // workspace root
        golden_dir.push("tests");
        golden_dir.push("golden");

        let mut kzd_files: Vec<std::path::PathBuf> = std::fs::read_dir(&golden_dir)
            .expect("golden dir must exist")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("kzd"))
            .collect();
        kzd_files.sort();
        assert!(
            kzd_files.len() >= 9,
            "expected at least 9 golden .kzd files, found {}",
            kzd_files.len()
        );

        for path in &kzd_files {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let src = std::fs::read_to_string(path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

            let formatted =
                format_kzd(&src).unwrap_or_else(|e| panic!("{name}: fmt failed: {e:?}"));
            let formatted2 = format_kzd(&formatted)
                .unwrap_or_else(|e| panic!("{name}: second fmt failed: {e:?}"));
            assert_eq!(
                formatted, formatted2,
                "{name}: fmt(fmt(x)) must equal fmt(x)"
            );

            let d1 = parse(&src).unwrap_or_else(|e| panic!("{name}: original parse: {e:?}"));
            let d2 = parse(&formatted).unwrap_or_else(|e| panic!("{name}: formatted parse: {e:?}"));
            assert_eq!(d1, d2, "{name}: fmt must preserve the parsed IR");
        }
    }

    // --- M3b follow-up 1: trailing comment on `diagram name {` line ---

    #[test]
    fn fmt_preserves_trailing_comment_on_diagram_line() {
        let src = "diagram d { // opening comment\n  a: \"A\"\n  b: \"B\"\n  a -> b\n}";
        let formatted = format_kzd(src).expect("should format");
        assert!(
            formatted.contains("// opening comment"),
            "trailing comment on diagram line should be preserved: {formatted}"
        );
        // The comment should appear before the first statement.
        let comment_pos = formatted.find("// opening comment").unwrap();
        let a_pos = formatted.find("a: \"A\"").unwrap();
        assert!(
            comment_pos < a_pos,
            "comment should appear before the first statement: {formatted}"
        );
        // Idempotent.
        let formatted2 = format_kzd(&formatted).expect("second format");
        assert_eq!(formatted, formatted2, "fmt must be idempotent");
    }
}
