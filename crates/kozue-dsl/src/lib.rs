//! DSL parser for kozue using chumsky 0.9, with ariadne diagnostics.
//!
//! The diagram kind is an explicit keyword at the start of the header:
//! `<kind> <name> { ... }` where kind is one of `graph`, `sequence`, `state`,
//! `class`, or `er`. There is no signal-based inference — the kind keyword
//! is the single source of truth, so grammar and diagnostics for each kind
//! are precise from the very first token.
//!
//! Grammar:
//! ```text
//! graph <name> {
//!   // line comments are allowed anywhere
//!   direction down|right|up|left
//!   <id>: "label"
//!   <a> -> <b> : "label"
//! }
//!
//! sequence <name> {
//!   participant <id>: "label"
//!   <a> -> <b> : "label"
//!   <a> --> <b> : "label"
//! }
//!
//! state <name> {
//!   state <id>: "label"
//!   [*] -> <id>
//!   <a> -> <b> : "label"
//! }
//!
//! class <name> {
//!   class Order {
//!     +id: Int
//!     +submit(): void
//!   }
//!   Customer "1" o-- "*" Order : "places"
//! }
//!
//! er <name> {
//!   entity Customer {
//!     id: Int PK
//!     name: String
//!   }
//!   Customer ||--o{ Order : "places"
//! }
//! ```

mod class_dsl;
mod er_dsl;

use ariadne::{Label, Report, ReportKind, Source};
use chumsky::prelude::*;
use kozue_ir::{
    ArrowType, Diagram, Direction, Edge, ElementId, Endpoint, GraphDiagram, IrDocument, LineStyle,
    LineWeight, Message, Node, NodeKind, Participant, SequenceDiagram, SequenceItem, State,
    StateDiagram, Transition,
};

/// A parsed statement inside a diagram body.
#[derive(Debug, Clone)]
enum Stmt {
    Direction(Direction, std::ops::Range<usize>),
    Node {
        id: String,
        id_span: std::ops::Range<usize>,
        shape: Option<ParsedNodeShape>,
        label: Option<String>,
        /// Span of the string literal (including quotes), if present.
        label_lit_span: Option<std::ops::Range<usize>>,
    },
    Edge(EdgeStmt),
    DashedEdge(EdgeStmt),
    /// `a --- b` — undirected graph edge.
    UndirectedEdge(EdgeStmt),
    /// `a <-> b` — bidirectional graph edge.
    BidirectionalEdge(EdgeStmt),
    Participant {
        id: String,
        id_span: std::ops::Range<usize>,
        label: Option<String>,
        /// Span of the string literal (including quotes), if present.
        label_lit_span: Option<std::ops::Range<usize>>,
    },
    DirectionError(std::ops::Range<usize>),
    StateDecl {
        id: String,
        id_span: std::ops::Range<usize>,
        label: Option<String>,
        label_lit_span: Option<std::ops::Range<usize>>,
    },
    StateTransition(StateTransStmt),
}

#[derive(Debug, Clone)]
enum ParsedNodeShape {
    Known(NodeKind, std::ops::Range<usize>),
    Unknown(String, std::ops::Range<usize>),
}

impl ParsedNodeShape {
    fn span(&self) -> &std::ops::Range<usize> {
        match self {
            ParsedNodeShape::Known(_, span) | ParsedNodeShape::Unknown(_, span) => span,
        }
    }
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
    /// `line <style>` / `weight <weight>` modifiers, in source order. Only
    /// meaningful for graph diagrams; state/sequence diagrams reject any
    /// non-empty modifier list. Order-independent, last-wins per kind.
    modifiers: Vec<EdgeModifier>,
}

/// A parsed `line <style>` edge modifier value.
#[derive(Debug, Clone)]
enum ParsedLineMod {
    Known(LineStyle, std::ops::Range<usize>),
    Unknown(String, std::ops::Range<usize>),
}

impl ParsedLineMod {
    fn span(&self) -> &std::ops::Range<usize> {
        match self {
            ParsedLineMod::Known(_, span) | ParsedLineMod::Unknown(_, span) => span,
        }
    }
}

/// A parsed `weight <weight>` edge modifier value.
#[derive(Debug, Clone)]
enum ParsedWeightMod {
    Known(LineWeight, std::ops::Range<usize>),
    Unknown(String, std::ops::Range<usize>),
}

impl ParsedWeightMod {
    fn span(&self) -> &std::ops::Range<usize> {
        match self {
            ParsedWeightMod::Known(_, span) | ParsedWeightMod::Unknown(_, span) => span,
        }
    }
}

/// A single edge presentation modifier (`line ...` or `weight ...`).
#[derive(Debug, Clone)]
enum EdgeModifier {
    Line(ParsedLineMod),
    Weight(ParsedWeightMod),
}

impl EdgeModifier {
    fn span(&self) -> &std::ops::Range<usize> {
        match self {
            EdgeModifier::Line(m) => m.span(),
            EdgeModifier::Weight(m) => m.span(),
        }
    }
}

/// Byte span covering every modifier attached to an edge statement, for
/// diagnostics that reject the whole modifier block (e.g. in state/sequence
/// diagrams). Falls back to the end of the target identifier if there are no
/// modifiers (callers should only invoke this when `modifiers` is non-empty).
fn edge_modifiers_span(e: &EdgeStmt) -> std::ops::Range<usize> {
    let start = e
        .modifiers
        .iter()
        .map(|m| m.span().start)
        .min()
        .unwrap_or(e.to_span.end);
    let end = e
        .modifiers
        .iter()
        .map(|m| m.span().end)
        .max()
        .unwrap_or(e.to_span.end);
    start..end
}

/// Resolve a modifier list to effective (line, weight) values, applying
/// last-wins semantics and defaulting to Solid/Normal. Assumes all modifiers
/// are `Known` (callers running after successful semantic validation, e.g.
/// the formatter, are guaranteed this since unknown modifiers are a build
/// error).
fn resolve_edge_modifiers(modifiers: &[EdgeModifier]) -> (LineStyle, LineWeight) {
    let mut line = LineStyle::Solid;
    let mut weight = LineWeight::Normal;
    for modifier in modifiers {
        match modifier {
            EdgeModifier::Line(ParsedLineMod::Known(value, _)) => line = *value,
            EdgeModifier::Weight(ParsedWeightMod::Known(value, _)) => weight = *value,
            _ => {}
        }
    }
    (line, weight)
}

/// A state endpoint: either `[*]` or a state ID.
#[derive(Debug, Clone)]
enum RawEndpoint {
    Pseudo,
    Id(String),
}

#[derive(Debug, Clone)]
struct StateTransStmt {
    from: RawEndpoint,
    from_span: std::ops::Range<usize>,
    to: RawEndpoint,
    to_span: std::ops::Range<usize>,
    label: Option<String>,
    label_lit_span: Option<std::ops::Range<usize>>,
    /// True if written with `-->` (dashed). Dashed transitions are not valid in
    /// state diagrams; we parse them anyway so `build_state_diagram` can emit an
    /// explicit diagnostic instead of a generic syntax error.
    dashed: bool,
    /// Span of the arrow token, for the dashed-not-supported diagnostic.
    arrow_span: std::ops::Range<usize>,
}

/// The diagram kind, determined by the required header keyword. `class` and
/// `er` are parsed entirely separately (see [`class_dsl`] / [`er_dsl`]) and
/// never produce an [`Ast`] of this shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiagramKind {
    Graph,
    Sequence,
    State,
}

#[derive(Debug, Clone)]
struct Ast {
    kind: DiagramKind,
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
    // direction statement: `direction down|right|up|left`
    let direction_kw = text::keyword("direction").padded_by(kzd_ws());
    let direction_val = text::keyword("down")
        .to(Direction::Down)
        .or(text::keyword("right").to(Direction::Right))
        .or(text::keyword("up").to(Direction::Up))
        .or(text::keyword("left").to(Direction::Left));

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

    // state declaration: `state id` or `state id: "label"`
    let state_decl = text::keyword("state")
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
            Stmt::StateDecl {
                id,
                id_span,
                label,
                label_lit_span,
            }
        });

    // [*] pseudostate token parser — captures span before padding.
    let pseudo_inner = just('[')
        .then(just('*'))
        .then(just(']'))
        .map_with_span(|_, span| (RawEndpoint::Pseudo, span));
    let pseudo = pseudo_inner.padded_by(kzd_ws());

    let id_endpoint = ident_spanned().map(|(id, span)| (RawEndpoint::Id(id), span));

    // Arrow between pseudostate endpoints: solid `->` or dashed `-->`. We accept
    // the dashed form so a wrong `[*] --> s` yields an explicit "dashed edges are
    // not supported" diagnostic rather than a generic syntax error. `-->` must be
    // tried first since it shares the `->` suffix.
    let state_arrow = just("-->")
        .to(true)
        .or(just("->").to(false))
        .map_with_span(|dashed, span| (dashed, span))
        .padded_by(kzd_ws());

    // Transitions with [*] on the left: `[*] -> id` or `[*] -> [*]`
    let pseudo_clone = pseudo.clone();
    let id_ep_clone = id_endpoint.clone();
    let state_trans_pseudo_left = pseudo
        .clone()
        .then(state_arrow.clone())
        .then(pseudo_clone.or(id_ep_clone))
        .then(
            just(':')
                .padded_by(kzd_ws())
                .ignore_then(string_lit_spanned())
                .or_not(),
        )
        .map(
            |((((from, from_span), (dashed, arrow_span)), (to, to_span)), label_opt)| {
                let (label, label_lit_span) = match label_opt {
                    Some((l, s)) => (Some(l), Some(s)),
                    None => (None, None),
                };
                Stmt::StateTransition(StateTransStmt {
                    from,
                    from_span,
                    to,
                    to_span,
                    label,
                    label_lit_span,
                    dashed,
                    arrow_span,
                })
            },
        );

    // Transitions with [*] on the right: `id -> [*]`
    let state_trans_pseudo_right = id_endpoint
        .clone()
        .then(state_arrow.clone())
        .then(pseudo.clone())
        .then(
            just(':')
                .padded_by(kzd_ws())
                .ignore_then(string_lit_spanned())
                .or_not(),
        )
        .map(
            |((((from, from_span), (dashed, arrow_span)), (to, to_span)), label_opt)| {
                let (label, label_lit_span) = match label_opt {
                    Some((l, s)) => (Some(l), Some(s)),
                    None => (None, None),
                };
                Stmt::StateTransition(StateTransStmt {
                    from,
                    from_span,
                    to,
                    to_span,
                    label,
                    label_lit_span,
                    dashed,
                    arrow_span,
                })
            },
        );

    // Edge presentation modifiers: `line solid|dashed|dotted` and
    // `weight normal|thick`, order-independent, last-wins (resolved later in
    // the build functions / formatter). Only meaningful for graph diagrams;
    // state/sequence diagrams reject a non-empty modifier list outright.
    let line_mod = text::keyword("line")
        .padded_by(kzd_ws())
        .ignore_then(ident_spanned())
        .map(|(name, span)| {
            EdgeModifier::Line(match name.as_str() {
                "solid" => ParsedLineMod::Known(LineStyle::Solid, span),
                "dashed" => ParsedLineMod::Known(LineStyle::Dashed, span),
                "dotted" => ParsedLineMod::Known(LineStyle::Dotted, span),
                _ => ParsedLineMod::Unknown(name, span),
            })
        });
    let weight_mod = text::keyword("weight")
        .padded_by(kzd_ws())
        .ignore_then(ident_spanned())
        .map(|(name, span)| {
            EdgeModifier::Weight(match name.as_str() {
                "normal" => ParsedWeightMod::Known(LineWeight::Normal, span),
                "thick" => ParsedWeightMod::Known(LineWeight::Thick, span),
                _ => ParsedWeightMod::Unknown(name, span),
            })
        });
    let edge_modifiers = line_mod.or(weight_mod).repeated();

    let label_suffix = just(':')
        .padded_by(kzd_ws())
        .ignore_then(string_lit_spanned())
        .or_not();

    // Dashed edge: `a --> b` optionally `: "label"`
    let dashed_edge = ident_spanned()
        .then_ignore(just("-->").padded_by(kzd_ws()))
        .then(ident_spanned())
        .then(edge_modifiers.clone())
        .then(label_suffix.clone())
        .map(
            |((((from, from_span), (to, to_span)), modifiers), label_opt)| {
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
                    modifiers,
                })
            },
        );

    // Solid edge: `a -> b` optionally `(line ... | weight ...)*` and `: "label"`.
    let edge = ident_spanned()
        .then_ignore(just("->").padded_by(kzd_ws()))
        .then(ident_spanned())
        .then(edge_modifiers.clone())
        .then(label_suffix.clone())
        .map(
            |((((from, from_span), (to, to_span)), modifiers), label_opt)| {
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
                    modifiers,
                })
            },
        );

    // Undirected edge: `a --- b` optionally `(line ... | weight ...)*` and `: "label"`.
    let undirected_edge = ident_spanned()
        .then_ignore(just("---").padded_by(kzd_ws()))
        .then(ident_spanned())
        .then(edge_modifiers.clone())
        .then(label_suffix.clone())
        .map(
            |((((from, from_span), (to, to_span)), modifiers), label_opt)| {
                let (label, label_lit_span) = match label_opt {
                    Some((l, s)) => (Some(l), Some(s)),
                    None => (None, None),
                };
                Stmt::UndirectedEdge(EdgeStmt {
                    from,
                    from_span,
                    to,
                    to_span,
                    label,
                    label_lit_span,
                    modifiers,
                })
            },
        );

    // Bidirectional edge: `a <-> b` optionally `(line ... | weight ...)*` and `: "label"`.
    let bidirectional_edge = ident_spanned()
        .then_ignore(just("<->").padded_by(kzd_ws()))
        .then(ident_spanned())
        .then(edge_modifiers.clone())
        .then(label_suffix.clone())
        .map(
            |((((from, from_span), (to, to_span)), modifiers), label_opt)| {
                let (label, label_lit_span) = match label_opt {
                    Some((l, s)) => (Some(l), Some(s)),
                    None => (None, None),
                };
                Stmt::BidirectionalEdge(EdgeStmt {
                    from,
                    from_span,
                    to,
                    to_span,
                    label,
                    label_lit_span,
                    modifiers,
                })
            },
        );

    // Node: `id`, `id: "label"`, or `id shape rectangle|rounded: "label"`.
    let node_shape = text::keyword("shape")
        .padded_by(kzd_ws())
        .ignore_then(ident_spanned())
        .map(|(name, span)| match name.as_str() {
            "rectangle" => ParsedNodeShape::Known(NodeKind::Rectangle, span),
            "rounded" => ParsedNodeShape::Known(NodeKind::RoundedRectangle, span),
            "circle" => ParsedNodeShape::Known(NodeKind::Circle, span),
            "diamond" => ParsedNodeShape::Known(NodeKind::Diamond, span),
            _ => ParsedNodeShape::Unknown(name, span),
        });
    let node = ident_spanned()
        .then(node_shape.or_not())
        .then(
            just(':')
                .padded_by(kzd_ws())
                .ignore_then(string_lit_spanned())
                .or_not(),
        )
        .map(|(((id, id_span), shape), label_opt)| {
            let (label, label_lit_span) = match label_opt {
                Some((l, s)) => (Some(l), Some(s)),
                None => (None, None),
            };
            Stmt::Node {
                id,
                id_span,
                shape,
                label,
                label_lit_span,
            }
        });

    let stmt = direction
        .or(participant)
        .or(state_decl)
        .or(state_trans_pseudo_left)
        .or(state_trans_pseudo_right)
        .or(dashed_edge)
        .or(bidirectional_edge)
        .or(undirected_edge)
        .or(edge)
        .or(node);
    let body = stmt.repeated().padded_by(kzd_ws());

    // The header keyword selects the diagram kind; there is no signal-based
    // inference. `class`/`er` are handled entirely outside this grammar (see
    // `class_dsl`/`er_dsl`), so they are not alternatives here.
    let header_kind = text::keyword("graph")
        .to(DiagramKind::Graph)
        .or(text::keyword("sequence").to(DiagramKind::Sequence))
        .or(text::keyword("state").to(DiagramKind::State))
        .padded_by(kzd_ws());

    header_kind
        .then(
            text::ident()
                .padded_by(kzd_ws())
                .map_with_span(|s, span| (s, span)),
        )
        .then_ignore(just('{').padded_by(kzd_ws()))
        .then(body)
        .then_ignore(just('}').padded_by(kzd_ws()))
        .then_ignore(end())
        .map(|((kind, (name, name_span)), stmts)| Ast {
            kind,
            name,
            name_span,
            stmts,
        })
}

/// Scan past leading whitespace and `//` comments and return the first
/// identifier-like token (the header keyword) along with its byte span.
/// Returns `None` if the source has no such token (e.g. empty input).
pub(crate) fn peek_header_keyword(src: &str) -> Option<(&str, std::ops::Range<usize>)> {
    let mut idx = 0usize;
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

/// Parse source text into a semantic [`Diagram`], collecting errors.
///
/// The header keyword (`graph`/`sequence`/`state`/`class`/`er`) determines
/// the diagram kind with no signal-based inference: `class`/`er` dispatch to
/// their own dedicated parsers ([`class_dsl`]/[`er_dsl`]), while
/// `graph`/`sequence`/`state` share the chumsky grammar below.
fn parse_diagram(src: &str) -> Result<Diagram, Vec<CompileError>> {
    let Some((kw, kw_span)) = peek_header_keyword(src) else {
        return Err(vec![CompileError {
            message: "expected diagram kind keyword (graph|sequence|state|class|er)".to_string(),
            span: 0..src.len().max(1),
            secondary: None,
        }]);
    };

    match kw {
        "graph" | "sequence" | "state" => {
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
        "class" => class_dsl::parse(src),
        "er" => er_dsl::parse(src),
        other => Err(vec![CompileError {
            message: format!(
                "expected diagram kind keyword (graph|sequence|state|class|er), got `{other}`"
            ),
            span: kw_span,
            secondary: None,
        }]),
    }
}

/// Parse source text into a versioned semantic IR document.
pub fn parse_document(src: &str) -> Result<IrDocument, Vec<CompileError>> {
    let diagram = parse_diagram(src)?;
    let mut document = IrDocument::new(diagram);
    document.metadata.name = header_name(src);
    Ok(document)
}

/// Parse source text into a semantic [`Diagram`].
///
/// This compatibility API discards document metadata. Use [`parse_document`]
/// when the native diagram name must be retained.
pub fn parse(src: &str) -> Result<Diagram, Vec<CompileError>> {
    parse_document(src).map(IrDocument::into_diagram)
}

fn header_name(src: &str) -> Option<String> {
    let (_, keyword_span) = peek_header_keyword(src)?;
    let mut rest = &src[keyword_span.end..];
    loop {
        rest = rest.trim_start();
        if !rest.starts_with("//") {
            break;
        }
        rest = &rest[rest.find('\n')? + 1..];
    }

    let end = rest
        .char_indices()
        .find(|&(_, c)| !(c.is_alphanumeric() || c == '_'))
        .map(|(offset, _)| offset)
        .unwrap_or(rest.len());
    (end != 0).then(|| rest[..end].to_string())
}

/// Dispatch to the appropriate builder for the AST's (already-determined)
/// kind. No signal-based inference: the header keyword alone selects the
/// builder, and each builder explicitly rejects statement kinds that don't
/// belong to it.
fn build_diagram(ast: Ast, src: &str) -> Result<Diagram, Vec<CompileError>> {
    match ast.kind {
        DiagramKind::Graph => build_graph_diagram(ast, src),
        DiagramKind::Sequence => build_sequence_diagram(ast, src),
        DiagramKind::State => build_state_diagram(ast, src),
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
                    message: "expected `down`, `right`, `up`, or `left` after `direction`"
                        .to_string(),
                    span: span.clone(),
                    secondary: None,
                });
            }
            Stmt::Node {
                id,
                id_span,
                shape,
                label,
                label_lit_span,
            } => {
                if graph.nodes.contains_key(id.as_str()) {
                    errors.push(CompileError {
                        message: format!("duplicate node declaration `{}`", id),
                        span: id_span.clone(),
                        secondary: first_decl_spans
                            .get(id)
                            .map(|s| (s.clone(), "first declared here".to_string())),
                    });
                    continue;
                }
                let kind = match shape {
                    Some(ParsedNodeShape::Known(kind, _)) => kind.clone(),
                    Some(ParsedNodeShape::Unknown(name, span)) => {
                        errors.push(CompileError {
                            message: format!(
                                "unknown node shape `{name}`; expected `rectangle`, `rounded`, `circle`, or `diamond`"
                            ),
                            span: span.clone(),
                            secondary: None,
                        });
                        continue;
                    }
                    None => NodeKind::Default,
                };
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
                graph.nodes.insert(
                    id.clone().into(),
                    Node::with_kind(id.clone(), label_str, kind),
                );
            }
            Stmt::DashedEdge(e) => {
                errors.push(CompileError {
                    message: "`-->` (dashed edge) is only valid in sequence diagrams; use `->` for graph diagrams".to_string(),
                    span: e.from_span.start..e.to_span.end,
                    secondary: None,
                });
            }
            Stmt::Participant { id_span, .. } => {
                errors.push(CompileError {
                    message: "`participant` declarations are not valid in graph diagrams; use plain `<id>` node declarations".to_string(),
                    span: id_span.clone(),
                    secondary: None,
                });
            }
            Stmt::StateDecl { id_span, .. } => {
                errors.push(CompileError {
                    message: "`state` declarations are not valid in graph diagrams; use plain `<id>` node declarations".to_string(),
                    span: id_span.clone(),
                    secondary: None,
                });
            }
            Stmt::StateTransition(t) => {
                errors.push(CompileError {
                    message: "`[*]` pseudostate transitions are only valid in state diagrams"
                        .to_string(),
                    span: t.from_span.start..t.to_span.end,
                    secondary: None,
                });
            }
            Stmt::Edge(_) | Stmt::UndirectedEdge(_) | Stmt::BidirectionalEdge(_) => {}
        }
    }
    graph.direction = direction;

    for stmt in &ast.stmts {
        let (e, arrow, from_arrow) = match stmt {
            Stmt::Edge(e) => (e, ArrowType::Triangle, ArrowType::None),
            Stmt::UndirectedEdge(e) => (e, ArrowType::None, ArrowType::None),
            Stmt::BidirectionalEdge(e) => (e, ArrowType::Triangle, ArrowType::Triangle),
            _ => continue,
        };

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
            if !graph.nodes.contains_key(endpoint.as_str()) {
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

        let mut line = LineStyle::Solid;
        let mut weight = LineWeight::Normal;
        for modifier in &e.modifiers {
            match modifier {
                EdgeModifier::Line(ParsedLineMod::Known(value, _)) => line = *value,
                EdgeModifier::Line(ParsedLineMod::Unknown(name, span)) => {
                    errors.push(CompileError {
                        message: format!(
                            "unknown edge line style `{name}`; expected `solid`, `dashed`, or `dotted`"
                        ),
                        span: span.clone(),
                        secondary: None,
                    });
                }
                EdgeModifier::Weight(ParsedWeightMod::Known(value, _)) => weight = *value,
                EdgeModifier::Weight(ParsedWeightMod::Unknown(name, span)) => {
                    errors.push(CompileError {
                        message: format!(
                            "unknown edge weight `{name}`; expected `normal` or `thick`"
                        ),
                        span: span.clone(),
                        secondary: None,
                    });
                }
            }
        }

        graph.edges.push(Edge::with_presentation(
            e.from.clone(),
            e.to.clone(),
            e.label.clone(),
            arrow,
            from_arrow,
            line,
            weight,
        ));
    }

    if errors.is_empty() {
        Ok(Diagram::Graph(graph))
    } else {
        Err(errors)
    }
}

/// Build a [`StateDiagram`] from the AST.
fn build_state_diagram(ast: Ast, src: &str) -> Result<Diagram, Vec<CompileError>> {
    let mut diagram = StateDiagram::new();
    let mut errors: Vec<CompileError> = Vec::new();
    let mut first_decl_spans: std::collections::BTreeMap<String, std::ops::Range<usize>> =
        std::collections::BTreeMap::new();

    // Process explicit state declarations.
    for stmt in &ast.stmts {
        match stmt {
            Stmt::Direction(_, span) => {
                errors.push(CompileError {
                    message: "`direction` is not valid in state diagrams".to_string(),
                    span: span.clone(),
                    secondary: None,
                });
            }
            Stmt::DirectionError(span) => {
                errors.push(CompileError {
                    message: "expected `down`, `right`, `up`, or `left` after `direction`"
                        .to_string(),
                    span: span.clone(),
                    secondary: None,
                });
            }
            Stmt::DashedEdge(e) => {
                errors.push(CompileError {
                    message: "dashed edges (`-->`) are not supported in state diagrams; use `->` for transitions".to_string(),
                    span: e.from_span.start..e.to_span.end,
                    secondary: None,
                });
            }
            Stmt::UndirectedEdge(e) => {
                errors.push(CompileError {
                    message: "undirected edges (`---`) are not supported in state diagrams; use `->` for transitions".to_string(),
                    span: e.from_span.start..e.to_span.end,
                    secondary: None,
                });
            }
            Stmt::BidirectionalEdge(e) => {
                errors.push(CompileError {
                    message: "bidirectional edges (`<->`) are not supported in state diagrams; use `->` for transitions".to_string(),
                    span: e.from_span.start..e.to_span.end,
                    secondary: None,
                });
            }
            Stmt::StateDecl {
                id,
                id_span,
                label,
                label_lit_span,
            } => {
                if diagram.states.contains_key(id.as_str()) {
                    errors.push(CompileError {
                        message: format!("duplicate state declaration `{}`", id),
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
                diagram
                    .states
                    .insert(id.clone().into(), State::new(id.clone(), label_str));
            }
            Stmt::Node { id_span, shape, .. } => {
                errors.push(CompileError {
                    message: if shape.is_some() {
                        "node shape declarations are only valid in graph diagrams".to_string()
                    } else {
                        "plain node declarations are not valid in state diagrams; use `state <id>`"
                            .to_string()
                    },
                    span: shape
                        .as_ref()
                        .map(|shape| shape.span().clone())
                        .unwrap_or_else(|| id_span.clone()),
                    secondary: None,
                });
            }
            Stmt::Participant { id_span, .. } => {
                errors.push(CompileError {
                    message: "`participant` declarations are not valid in state diagrams"
                        .to_string(),
                    span: id_span.clone(),
                    secondary: None,
                });
            }
            Stmt::Edge(e) => {
                if !e.modifiers.is_empty() {
                    errors.push(CompileError {
                        message:
                            "`line`/`weight` edge modifiers are not supported in state diagrams"
                                .to_string(),
                        span: edge_modifiers_span(e),
                        secondary: None,
                    });
                }
            }
            Stmt::StateTransition(_) => {}
        }
    }

    // Process transitions.
    for stmt in &ast.stmts {
        match stmt {
            Stmt::StateTransition(t) => {
                // Dashed transitions are not valid in state diagrams. We parsed
                // the `-->` form only to give this explicit diagnostic.
                if t.dashed {
                    errors.push(CompileError {
                        message: "dashed edges (`-->`) are not supported in state diagrams; use `->` for transitions".to_string(),
                        span: t.arrow_span.clone(),
                        secondary: None,
                    });
                    continue;
                }

                let from_ep = match &t.from {
                    RawEndpoint::Pseudo => Endpoint::Initial,
                    RawEndpoint::Id(id) => Endpoint::State(id.clone().into()),
                };
                let to_ep = match &t.to {
                    RawEndpoint::Pseudo => Endpoint::Final,
                    RawEndpoint::Id(id) => Endpoint::State(id.clone().into()),
                };

                // Validate: [*] -> [*] makes no sense.
                if matches!(from_ep, Endpoint::Initial) && matches!(to_ep, Endpoint::Final) {
                    errors.push(CompileError {
                        message: "`[*] -> [*]` is not valid; initial pseudostate cannot transition directly to final pseudostate".to_string(),
                        span: t.from_span.start..t.to_span.end,
                        secondary: None,
                    });
                    continue;
                }

                // Auto-declare referenced state IDs.
                if let Endpoint::State(id) = &from_ep {
                    if !diagram.states.contains_key(id) {
                        diagram
                            .states
                            .insert(id.clone(), State::new(id.clone(), id.to_string()));
                    }
                }
                if let Endpoint::State(id) = &to_ep {
                    if !diagram.states.contains_key(id) {
                        diagram
                            .states
                            .insert(id.clone(), State::new(id.clone(), id.to_string()));
                    }
                }

                if let Some(lit_span) = &t.label_lit_span {
                    if let Some(err_span) = find_invalid_escape_in_span(src, lit_span) {
                        errors.push(CompileError {
                            message: "invalid escape sequence in string literal (only `\\\"` and `\\\\` are supported)".to_string(),
                            span: err_span,
                            secondary: None,
                        });
                    }
                }

                diagram
                    .transitions
                    .push(Transition::new(from_ep, to_ep, t.label.clone()));
            }
            Stmt::Edge(e) => {
                // In a state diagram, plain `id -> id` edges are treated as state transitions.
                let from_ep = Endpoint::State(e.from.clone().into());
                let to_ep = Endpoint::State(e.to.clone().into());

                if !diagram.states.contains_key(e.from.as_str()) {
                    diagram.states.insert(
                        e.from.clone().into(),
                        State::new(e.from.clone(), e.from.clone()),
                    );
                }
                if !diagram.states.contains_key(e.to.as_str()) {
                    diagram
                        .states
                        .insert(e.to.clone().into(), State::new(e.to.clone(), e.to.clone()));
                }
                if let Some(lit_span) = &e.label_lit_span {
                    if let Some(err_span) = find_invalid_escape_in_span(src, lit_span) {
                        errors.push(CompileError {
                            message: "invalid escape sequence in string literal (only `\\\"` and `\\\\` are supported)".to_string(),
                            span: err_span,
                            secondary: None,
                        });
                    }
                }
                diagram
                    .transitions
                    .push(Transition::new(from_ep, to_ep, e.label.clone()));
            }
            _ => continue,
        }
    }

    if errors.is_empty() {
        Ok(Diagram::State(diagram))
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
                    message: "expected `down`, `right`, `up`, or `left` after `direction`"
                        .to_string(),
                    span: span.clone(),
                    secondary: None,
                });
                continue;
            }
            Stmt::Node { id_span, shape, .. } => {
                errors.push(CompileError {
                    message: if shape.is_some() {
                        "node shape declarations are only valid in graph diagrams".to_string()
                    } else {
                        "plain node declarations are not valid in sequence diagrams; use `participant <id>`".to_string()
                    },
                    span: shape
                        .as_ref()
                        .map(|shape| shape.span().clone())
                        .unwrap_or_else(|| id_span.clone()),
                    secondary: None,
                });
                continue;
            }
            Stmt::StateDecl { id_span, .. } => {
                errors.push(CompileError {
                    message: "`state` declarations are not valid in sequence diagrams".to_string(),
                    span: id_span.clone(),
                    secondary: None,
                });
                continue;
            }
            Stmt::StateTransition(t) => {
                errors.push(CompileError {
                    message: "`[*]` pseudostate transitions are only valid in state diagrams"
                        .to_string(),
                    span: t.from_span.start..t.to_span.end,
                    secondary: None,
                });
                continue;
            }
            Stmt::UndirectedEdge(e) => {
                errors.push(CompileError {
                    message: "undirected edges (`---`) are only valid in graph diagrams; use `->` or `-->` for sequence diagrams".to_string(),
                    span: e.from_span.start..e.to_span.end,
                    secondary: None,
                });
                continue;
            }
            Stmt::BidirectionalEdge(e) => {
                errors.push(CompileError {
                    message: "bidirectional edges (`<->`) are only valid in graph diagrams; use `->` or `-->` for sequence diagrams".to_string(),
                    span: e.from_span.start..e.to_span.end,
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
            if seq.participants.contains_key(id.as_str()) {
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
                .insert(id.clone().into(), Participant::new(id.clone(), label_str));
        }
    }

    for stmt in &ast.stmts {
        let (e, line_style) = match stmt {
            Stmt::Edge(e) => (e, LineStyle::Solid),
            Stmt::DashedEdge(e) => (e, LineStyle::Dashed),
            _ => continue,
        };

        if !e.modifiers.is_empty() {
            errors.push(CompileError {
                message: "`line`/`weight` edge modifiers are not supported in sequence diagrams"
                    .to_string(),
                span: edge_modifiers_span(e),
                secondary: None,
            });
            continue;
        }

        let mut valid = true;
        for (endpoint, span) in [(&e.from, &e.from_span), (&e.to, &e.to_span)] {
            if !seq.participants.contains_key(endpoint.as_str()) {
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
    candidates: impl Iterator<Item = &'a ElementId>,
) -> Option<&'a ElementId> {
    candidates
        .map(|c| (levenshtein(target, c.as_str()), c))
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
    // The canonical formatter currently covers graph/sequence/state only.
    // class/er use a separate scanner (see `class_dsl`/`er_dsl`) and have no
    // formatter yet, so surface a clear, actionable error rather than a
    // confusing chumsky parse failure on their header keyword.
    if let Some((kw, kw_span)) = peek_header_keyword(src) {
        if kw == "class" || kw == "er" {
            return Err(vec![CompileError {
                message: format!("`kozue fmt` does not yet support `{kw}` diagrams"),
                span: kw_span,
                secondary: None,
            }]);
        }
    }

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
            if matches!(
                s,
                Stmt::Node { .. } | Stmt::Participant { .. } | Stmt::StateDecl { .. }
            ) {
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
            if matches!(
                s,
                Stmt::Edge(_)
                    | Stmt::DashedEdge(_)
                    | Stmt::UndirectedEdge(_)
                    | Stmt::BidirectionalEdge(_)
                    | Stmt::StateTransition(_)
            ) {
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

    // `<kind> <name> {`
    let kind_kw = match ast.kind {
        DiagramKind::Graph => "graph",
        DiagramKind::Sequence => "sequence",
        DiagramKind::State => "state",
    };
    out.push_str(&format!("{kind_kw} {} {{", ast.name));
    out.push('\n');

    // Direction statement (and its leading standalone comments).
    let has_direction = direction_idx.is_some() || !direction_error_indices.is_empty();
    if let Some(idx) = direction_idx {
        let dir_str = match &ast.stmts[idx] {
            Stmt::Direction(Direction::Down, _) => "direction down",
            Stmt::Direction(Direction::Right, _) => "direction right",
            Stmt::Direction(Direction::Up, _) => "direction up",
            Stmt::Direction(Direction::Left, _) => "direction left",
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
            shape,
            label_lit_span,
            ..
        } => {
            let end = label_lit_span
                .as_ref()
                .map(|s| s.end)
                .or_else(|| shape.as_ref().map(|shape| shape.span().end))
                .unwrap_or(id_span.end);
            (id_span.start, end)
        }
        Stmt::Participant {
            id_span,
            label_lit_span,
            ..
        } => {
            let end = label_lit_span
                .as_ref()
                .map(|span| span.end)
                .unwrap_or(id_span.end);
            (id_span.start, end)
        }
        Stmt::Edge(e)
        | Stmt::DashedEdge(e)
        | Stmt::UndirectedEdge(e)
        | Stmt::BidirectionalEdge(e) => {
            let end = e
                .label_lit_span
                .as_ref()
                .map(|s| s.end)
                .or_else(|| e.modifiers.iter().map(|m| m.span().end).max())
                .unwrap_or(e.to_span.end);
            (e.from_span.start, end)
        }
        Stmt::StateDecl {
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
        Stmt::StateTransition(t) => {
            let end = t
                .label_lit_span
                .as_ref()
                .map(|s| s.end)
                .unwrap_or(t.to_span.end);
            let start = t.from_span.start.min(t.to_span.start);
            (start, end)
        }
    }
}

/// Format a declaration statement (Node or Participant).
fn format_decl_stmt(stmt: &Stmt) -> String {
    match stmt {
        Stmt::Node {
            id, shape, label, ..
        } => {
            let mut declaration = id.clone();
            if let Some(ParsedNodeShape::Known(kind, _)) = shape {
                match kind {
                    NodeKind::Default => {}
                    NodeKind::Rectangle => declaration.push_str(" shape rectangle"),
                    NodeKind::RoundedRectangle => declaration.push_str(" shape rounded"),
                    NodeKind::Circle => declaration.push_str(" shape circle"),
                    NodeKind::Diamond => declaration.push_str(" shape diamond"),
                    _ => unreachable!(),
                }
            }
            if let Some(label_str) = label {
                declaration.push_str(": ");
                declaration.push_str(&format_string_lit(label_str));
            }
            declaration
        }
        Stmt::Participant { id, label, .. } => {
            if let Some(label_str) = label {
                format!("participant {}: {}", id, format_string_lit(label_str))
            } else {
                format!("participant {}", id)
            }
        }
        Stmt::StateDecl { id, label, .. } => {
            if let Some(label_str) = label {
                format!("state {}: {}", id, format_string_lit(label_str))
            } else {
                format!("state {}", id)
            }
        }
        _ => String::new(),
    }
}

/// Token for a resolved [`LineStyle`] as used in `line <token>` modifiers.
fn line_style_token(style: LineStyle) -> &'static str {
    match style {
        LineStyle::Solid => "solid",
        LineStyle::Dashed => "dashed",
        LineStyle::Dotted => "dotted",
        _ => "solid",
    }
}

/// Token for a resolved [`LineWeight`] as used in `weight <token>` modifiers.
fn line_weight_token(weight: LineWeight) -> &'static str {
    match weight {
        LineWeight::Normal => "normal",
        LineWeight::Thick => "thick",
        _ => "normal",
    }
}

/// Format the canonical `a <op> b [line ...] [weight ...] [: "label"]` body
/// shared by the three graph edge arrow tokens.
fn format_graph_edge(e: &EdgeStmt, op: &str) -> String {
    let (line, weight) = resolve_edge_modifiers(&e.modifiers);
    let mut out = format!("{} {} {}", e.from, op, e.to);
    if line != LineStyle::Solid {
        out.push_str(" line ");
        out.push_str(line_style_token(line));
    }
    if weight != LineWeight::Normal {
        out.push_str(" weight ");
        out.push_str(line_weight_token(weight));
    }
    if let Some(label_str) = &e.label {
        out.push_str(" : ");
        out.push_str(&format_string_lit(label_str));
    }
    out
}

/// Format an edge statement (Edge, DashedEdge, UndirectedEdge, BidirectionalEdge).
fn format_edge_stmt(stmt: &Stmt) -> String {
    match stmt {
        Stmt::Edge(e) => format_graph_edge(e, "->"),
        Stmt::UndirectedEdge(e) => format_graph_edge(e, "---"),
        Stmt::BidirectionalEdge(e) => format_graph_edge(e, "<->"),
        Stmt::DashedEdge(e) => {
            if let Some(label_str) = &e.label {
                format!("{} --> {} : {}", e.from, e.to, format_string_lit(label_str))
            } else {
                format!("{} --> {}", e.from, e.to)
            }
        }
        Stmt::StateTransition(t) => {
            let from_str = match &t.from {
                RawEndpoint::Pseudo => "[*]".to_string(),
                RawEndpoint::Id(id) => id.clone(),
            };
            let to_str = match &t.to {
                RawEndpoint::Pseudo => "[*]".to_string(),
                RawEndpoint::Id(id) => id.clone(),
            };
            if let Some(label_str) = &t.label {
                format!(
                    "{} -> {} : {}",
                    from_str,
                    to_str,
                    format_string_lit(label_str)
                )
            } else {
                format!("{} -> {}", from_str, to_str)
            }
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_document_preserves_all_native_header_names() {
        let cases = [
            ("graph graph_name {}", "graph_name"),
            ("sequence sequence_name {}", "sequence_name"),
            ("state state_name {}", "state_name"),
            ("class class_name {\n}\n", "class_name"),
            ("er er_name {\n}\n", "er_name"),
        ];

        for (source, expected_name) in cases {
            let document = parse_document(source)
                .unwrap_or_else(|errors| panic!("failed to parse {expected_name}: {errors:?}"));
            assert_eq!(document.metadata.name.as_deref(), Some(expected_name));
            assert!(document.extensions.is_empty());
        }
    }

    #[test]
    fn legacy_parse_returns_the_same_diagram_as_parse_document() {
        let source = "graph named { a }";
        assert_eq!(
            parse(source).unwrap(),
            parse_document(source).unwrap().into_diagram()
        );
    }

    #[test]
    fn parses_basic_diagram() {
        let src = r#"graph flow {
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
        let src = "graph d { a\n a -> b\n b }";
        let d = parse(src).expect("should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.nodes["a"].label, "a");
    }

    #[test]
    fn undeclared_node_is_error() {
        let src = "graph d {\n a: \"A\"\n a -> missing\n}";
        let err = parse(src).expect_err("should fail");
        assert!(err.iter().any(|e| e.message.contains("unknown node")));
    }

    #[test]
    fn undeclared_node_suggests_similar_name() {
        let src = "graph d {\n proc: \"P\"\n start: \"S\"\n start -> prok\n}";
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
        let src = "graph d { direction right\n a\n b\n a -> b }";
        let d = parse(src).expect("should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.direction, Direction::Right);
    }

    #[test]
    fn direction_up_and_left() {
        for (keyword, expected) in [("up", Direction::Up), ("left", Direction::Left)] {
            let source = format!("graph d {{ direction {keyword}\n a\n b\n a -> b }}");
            let Diagram::Graph(graph) = parse(&source).expect("should parse") else {
                panic!("expected graph")
            };
            assert_eq!(graph.direction, expected);
        }
    }

    #[test]
    fn graph_node_shapes_parse_and_unknown_shape_has_exact_span() {
        let source = "graph d {\n a\n b shape rectangle\n c shape rounded: \"See\"\n d shape circle\n e shape diamond: \"Decide\"\n}";
        let Diagram::Graph(graph) = parse(source).expect("shapes should parse") else {
            panic!("expected graph")
        };
        assert_eq!(graph.nodes["a"].kind, NodeKind::Default);
        assert_eq!(graph.nodes["b"].kind, NodeKind::Rectangle);
        assert_eq!(graph.nodes["c"].kind, NodeKind::RoundedRectangle);
        assert_eq!(graph.nodes["c"].label, "See");
        assert_eq!(graph.nodes["d"].kind, NodeKind::Circle);
        assert_eq!(graph.nodes["e"].kind, NodeKind::Diamond);
        assert_eq!(graph.nodes["e"].label, "Decide");

        let invalid = "graph d { a shape capsule }";
        let errors = parse(invalid).expect_err("unknown shape must fail");
        let error = errors
            .iter()
            .find(|error| error.message.contains("unknown node shape"))
            .unwrap();
        let start = invalid.find("capsule").unwrap();
        assert_eq!(error.span, start..start + "capsule".len());
    }

    #[test]
    fn node_shapes_are_graph_only() {
        for source in [
            "sequence d { a shape rectangle }",
            "state d { a shape rounded }",
            "sequence d { a shape circle }",
            "state d { a shape diamond }",
        ] {
            let errors = parse(source).expect_err("shape must be graph-only");
            assert!(errors
                .iter()
                .any(|error| error.message.contains("only valid in graph diagrams")));
        }
    }

    #[test]
    fn graph_edge_presentation_tokens_parse() {
        let source = "graph d {\n a\n b\n c\n a -> b\n b --- c\n c <-> a\n}";
        let Diagram::Graph(graph) = parse(source).expect("should parse") else {
            panic!("expected graph")
        };
        assert_eq!(graph.edges.len(), 3);
        assert_eq!(graph.edges[0].arrow, ArrowType::Triangle);
        assert_eq!(graph.edges[0].from_arrow, ArrowType::None);
        assert_eq!(graph.edges[1].arrow, ArrowType::None);
        assert_eq!(graph.edges[1].from_arrow, ArrowType::None);
        assert_eq!(graph.edges[2].arrow, ArrowType::Triangle);
        assert_eq!(graph.edges[2].from_arrow, ArrowType::Triangle);
        for edge in &graph.edges {
            assert_eq!(edge.line, LineStyle::Solid);
            assert_eq!(edge.weight, kozue_ir::LineWeight::Normal);
        }
    }

    #[test]
    fn graph_edge_line_and_weight_modifiers_parse_last_wins() {
        let source = "graph d {\n a\n b\n a -> b line dashed weight thick\n}";
        let Diagram::Graph(graph) = parse(source).expect("should parse") else {
            panic!("expected graph")
        };
        assert_eq!(graph.edges[0].line, LineStyle::Dashed);
        assert_eq!(graph.edges[0].weight, kozue_ir::LineWeight::Thick);

        // Last-wins when repeated.
        let source2 =
            "graph d {\n a\n b\n a -> b line dashed line dotted weight thick weight normal\n}";
        let Diagram::Graph(graph2) = parse(source2).expect("should parse") else {
            panic!("expected graph")
        };
        assert_eq!(graph2.edges[0].line, LineStyle::Dotted);
        assert_eq!(graph2.edges[0].weight, kozue_ir::LineWeight::Normal);
    }

    #[test]
    fn graph_edge_modifiers_apply_to_all_arrow_tokens() {
        let source = "graph d {\n a\n b\n c\n a -> b line dotted\n b --- c weight thick\n c <-> a line dashed weight thick\n}";
        let Diagram::Graph(graph) = parse(source).expect("should parse") else {
            panic!("expected graph")
        };
        assert_eq!(graph.edges[0].line, LineStyle::Dotted);
        assert_eq!(graph.edges[1].weight, kozue_ir::LineWeight::Thick);
        assert_eq!(graph.edges[2].line, LineStyle::Dashed);
        assert_eq!(graph.edges[2].weight, kozue_ir::LineWeight::Thick);
    }

    #[test]
    fn graph_edge_unknown_line_style_is_error_with_exact_span() {
        let src = "graph d { a\n b\n a -> b line teal }";
        let errors = parse(src).expect_err("unknown line style must fail");
        let error = errors
            .iter()
            .find(|e| e.message.contains("unknown edge line style"))
            .unwrap();
        assert!(error
            .message
            .contains("unknown edge line style `teal`; expected `solid`, `dashed`, or `dotted`"));
        let start = src.find("teal").unwrap();
        assert_eq!(error.span, start..start + "teal".len());
    }

    #[test]
    fn graph_edge_unknown_weight_is_error_with_exact_span() {
        let src = "graph d { a\n b\n a -> b weight bold }";
        let errors = parse(src).expect_err("unknown weight must fail");
        let error = errors
            .iter()
            .find(|e| e.message.contains("unknown edge weight"))
            .unwrap();
        assert!(error
            .message
            .contains("unknown edge weight `bold`; expected `normal` or `thick`"));
        let start = src.find("bold").unwrap();
        assert_eq!(error.span, start..start + "bold".len());
    }

    #[test]
    fn state_rejects_new_edge_tokens_and_modifiers() {
        let cases = [
            ("state d { state a\n state b\n a --- b }", "undirected"),
            ("state d { state a\n state b\n a <-> b }", "bidirectional"),
            (
                "state d { state a\n state b\n a -> b line dashed }",
                "modifiers",
            ),
        ];
        for (src, label) in cases {
            let errors = parse(src).expect_err(&format!("{label} must be rejected in state"));
            assert!(
                errors.iter().any(|e| e.message.contains("state diagrams")),
                "{label}: got {errors:?}"
            );
        }
    }

    #[test]
    fn sequence_rejects_new_edge_tokens_and_modifiers() {
        let cases = [
            (
                "sequence d { participant a\n participant b\n a --- b }",
                "undirected",
            ),
            (
                "sequence d { participant a\n participant b\n a <-> b }",
                "bidirectional",
            ),
            (
                "sequence d { participant a\n participant b\n a -> b line dashed }",
                "modifiers",
            ),
        ];
        for (src, label) in cases {
            let errors = parse(src).expect_err(&format!("{label} must be rejected in sequence"));
            assert!(
                errors
                    .iter()
                    .any(|e| e.message.contains("sequence diagrams")),
                "{label}: got {errors:?}"
            );
        }
    }

    #[test]
    fn fmt_edge_presentation_canonical_output() {
        let source =
            "graph d {\n a\n b\n c\n a -> b\n b --- c line dotted\n c <-> a weight thick\n}";
        let formatted = format_kzd(source).expect("should format");
        assert!(formatted.contains("a -> b\n"));
        assert!(formatted.contains("b --- c line dotted\n"));
        assert!(formatted.contains("c <-> a weight thick\n"));
    }

    #[test]
    fn fmt_edge_presentation_is_idempotent() {
        let source = "graph d {\n a\n b\n c\n a -> b line dashed weight thick : \"L\"\n b --- c\n c <-> a line dotted\n}";
        let formatted = format_kzd(source).expect("should format");
        assert_eq!(format_kzd(&formatted).unwrap(), formatted);
    }

    #[test]
    fn fmt_default_presentation_edges_are_unchanged() {
        // Default-presentation directed edges must format exactly as before
        // this milestone, with no `line`/`weight` tokens emitted.
        let source = "graph d {\n a\n b\n a -> b : \"L\"\n}";
        let formatted = format_kzd(source).expect("should format");
        assert!(formatted.contains("a -> b : \"L\"\n"));
        assert!(!formatted.contains("line "));
        assert!(!formatted.contains("weight "));
    }

    #[test]
    fn direction_invalid_value_is_error() {
        let src = "graph d { direction dwn\n a\n b }";
        let err = parse(src).expect_err("should fail on invalid direction value");
        assert!(
            err.iter().any(|e| e
                .message
                .contains("expected `down`, `right`, `up`, or `left`")),
            "got: {err:?}"
        );
    }

    #[test]
    fn direction_missing_value_is_error() {
        let src = "graph d { direction }";
        let result = parse(src);
        assert!(
            result.is_err(),
            "should fail when direction value is missing"
        );
    }

    #[test]
    fn self_loop_is_error() {
        let src = "graph d { a\n a -> a }";
        let err = parse(src).expect_err("self-loop should be an error");
        assert!(
            err.iter()
                .any(|e| e.message.contains("self-loops are not yet supported")),
            "got: {err:?}"
        );
    }

    #[test]
    fn duplicate_node_is_error() {
        let src = "graph d { a: \"First\"\n a: \"Second\" }";
        let err = parse(src).expect_err("duplicate node should be an error");
        assert!(
            err.iter()
                .any(|e| e.message.contains("duplicate node declaration")),
            "got: {err:?}"
        );
    }

    #[test]
    fn string_escape_backslash_and_quote() {
        let src = r#"graph d { a: "say \"hello\" and \\" }"#;
        let d = parse(src).expect("should parse escaped strings");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.nodes["a"].label, r#"say "hello" and \"#);
    }

    #[test]
    fn invalid_escape_sequence_is_error() {
        let src = r#"graph d { a: "bad \n escape" }"#;
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
        let src = r#"sequence seq {
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
        let src = r#"sequence seq {
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
        let src = r#"sequence seq {
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
        let src = r#"sequence seq {
  participant a: "A"
  direction bogus
  a -> a
}"#;
        let err = parse(src).expect_err("bogus direction in sequence should be an error");
        assert!(
            err.iter().any(|e| e
                .message
                .contains("expected `down`, `right`, `up`, or `left`")),
            "got: {err:?}"
        );
    }

    // --- Issue 2: escape error deduplication ---

    #[test]
    fn invalid_escape_reported_once_per_label_not_multiplied() {
        let src = "sequence seq {\n  participant a: \"ok\"\n  participant b: \"bad \\n escape\"\n}";
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
        let src = "graph d { a: \"bad \\n\" b: \"also \\t bad\" a -> b }";
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
        let src = "graph d { a: \"First\"\n a: \"Second\" }";
        let err = parse(src).expect_err("duplicate node should be an error");
        let dup_err = err
            .iter()
            .find(|e| e.message.contains("duplicate node declaration"))
            .expect("should have duplicate error");
        // The second `a` starts after the newline at position 23.
        // In the source "graph d { a: \"First\"\n a: \"Second\" }"
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
        let src = "sequence seq {\n  participant a: \"A\"\n  participant a: \"B\"\n}";
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
        let src = "graph d {\n a: \"A\"\n a -> ghost\n}";
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
        let src = "graph d { a: \"ok\" b: \"bad \\n escape\" a -> b }";
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
        let src = "// a comment\ngraph d { a\n b\n a -> b }";
        let d = parse(src).expect("comment before diagram should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.nodes.len(), 2);
    }

    #[test]
    fn line_comment_inside_body() {
        let src = "graph d {\n  // standalone comment\n  a: \"A\"\n  b: \"B\"\n  a -> b\n}";
        let d = parse(src).expect("comment inside body should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.nodes.len(), 2);
    }

    #[test]
    fn trailing_comment_after_statement() {
        let src = "graph d {\n  a: \"A\"  // node A\n  b: \"B\"\n  a -> b\n}";
        let d = parse(src).expect("trailing comment should parse");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.nodes["a"].label, "A");
    }

    #[test]
    fn double_slash_inside_string_is_not_comment() {
        // `//` inside a string literal should not start a comment.
        let src = r#"graph d { a: "http://example.com" b: "B" a -> b }"#;
        let d = parse(src).expect("// inside string should not be a comment");
        let Diagram::Graph(g) = d else { panic!() };
        assert_eq!(g.nodes["a"].label, "http://example.com");
    }

    #[test]
    fn comment_does_not_affect_golden_parse() {
        // Source identical to chain.kzd but with added comments should produce
        // the same IR as the original.
        let src_no_comment = "graph chain {\n  direction down\n\n  a: \"入力\"\n  b: \"変換\"\n  c: \"出力\"\n\n  a -> b : \"read\"\n  b -> c : \"write\"\n}\n";
        let src_with_comment = "// Chain diagram\ngraph chain {\n  direction down  // layout direction\n\n  a: \"入力\"\n  b: \"変換\"\n  c: \"出力\"\n\n  a -> b : \"read\"\n  b -> c : \"write\"\n}\n";
        let d1 = parse(src_no_comment).expect("no-comment should parse");
        let d2 = parse(src_with_comment).expect("with-comment should parse");
        assert_eq!(d1, d2, "comments should not affect the parsed IR");
    }

    // --- M3a Part 3: Formatter tests ---

    #[test]
    fn fmt_simple_graph_is_canonical() {
        let src = "graph d{a:\"A\"\nb:\"B\"\na->b}";
        let formatted = format_kzd(src).expect("should format");
        // Must be parseable.
        parse(&formatted).expect("formatted output should parse");
        // Must be idempotent.
        let formatted2 = format_kzd(&formatted).expect("second format should succeed");
        assert_eq!(formatted, formatted2, "fmt must be idempotent");
    }

    #[test]
    fn fmt_idempotent_on_golden_chain() {
        let src = "graph chain {\n  direction down\n\n  a: \"入力\"\n  b: \"変換\"\n  c: \"出力\"\n\n  a -> b : \"read\"\n  b -> c : \"write\"\n}\n";
        let formatted = format_kzd(src).expect("should format");
        let formatted2 = format_kzd(&formatted).expect("second format should succeed");
        assert_eq!(formatted, formatted2, "fmt must be idempotent");
    }

    #[test]
    fn fmt_semantic_preservation() {
        let src = "graph chain {\n  direction down\n\n  a: \"入力\"\n  b: \"変換\"\n  c: \"出力\"\n\n  a -> b : \"read\"\n  b -> c : \"write\"\n}\n";
        let formatted = format_kzd(src).expect("should format");
        let d1 = parse(src).expect("original should parse");
        let d2 = parse(&formatted).expect("formatted should parse");
        assert_eq!(d1, d2, "fmt must preserve semantics");
    }

    #[test]
    fn fmt_syntax_error_returns_error() {
        let src = "graph d { bad syntax !!! }";
        let result = format_kzd(src);
        assert!(result.is_err(), "fmt on invalid source should return error");
    }

    #[test]
    fn fmt_preserves_trailing_comment() {
        let src = "graph d {\n  a: \"A\"  // node a\n  b: \"B\"\n  a -> b\n}";
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
        let src = "graph d {\n  // declarations\n  a: \"A\"\n  b: \"B\"\n  a -> b\n}";
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
        let src = "sequence seq {\n  participant a: \"Alice\"\n  participant b: \"Bob\"\n  a -> b : \"hello\"\n  b --> a : \"reply\"\n}\n";
        let formatted = format_kzd(src).expect("should format");
        let d1 = parse(src).expect("original should parse");
        let d2 = parse(&formatted).expect("formatted should parse");
        assert_eq!(d1, d2, "fmt must preserve sequence diagram semantics");
        let formatted2 = format_kzd(&formatted).expect("second format");
        assert_eq!(formatted, formatted2, "fmt must be idempotent");
    }

    #[test]
    fn fmt_direction_right() {
        let src = "graph p {\n  direction right\n  src: \"S\"\n  dst: \"D\"\n  src -> dst\n}\n";
        let formatted = format_kzd(src).expect("should format");
        assert!(
            formatted.contains("direction right"),
            "direction must be present"
        );
        let formatted2 = format_kzd(&formatted).expect("second format");
        assert_eq!(formatted, formatted2, "fmt must be idempotent");
    }

    #[test]
    fn fmt_direction_up_and_left_is_idempotent() {
        for keyword in ["up", "left"] {
            let source = format!(
                "graph p {{\n direction {keyword}\n src: \"S\"\n dst: \"D\"\n src -> dst\n}}"
            );
            let formatted = format_kzd(&source).expect("should format");
            assert!(formatted.contains(&format!("direction {keyword}")));
            assert_eq!(format_kzd(&formatted).expect("second format"), formatted);
        }
    }

    #[test]
    fn fmt_node_shapes_is_idempotent() {
        let source = "graph shapes { a shape rectangle\n b shape rounded : \"Bee\"\n c shape circle\n d shape diamond : \"Dee\"\n a -> b }";
        let formatted = format_kzd(source).expect("should format");
        assert!(formatted.contains("a shape rectangle"));
        assert!(formatted.contains("b shape rounded: \"Bee\""));
        assert!(formatted.contains("c shape circle"));
        assert!(formatted.contains("d shape diamond: \"Dee\""));
        assert_eq!(format_kzd(&formatted).unwrap(), formatted);
    }

    #[test]
    fn fmt_comment_before_edge_section() {
        // Standalone comment before the first edge must appear before that edge.
        let src = "graph d {\n  // nodes section\n  a: \"A\"\n  b: \"B\"\n  // edges section\n  a -> b\n}";
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
            kzd_files.len() >= 10,
            "expected at least 10 golden .kzd files, found {}",
            kzd_files.len()
        );

        for path in &kzd_files {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let src = std::fs::read_to_string(path)
                .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));

            // The canonical formatter covers graph/sequence/state only; class/er
            // diagrams parse but have no formatter yet (they use a separate
            // scanner), so they are excluded from the fmt-idempotency sweep.
            if matches!(
                peek_header_keyword(&src).map(|(kw, _)| kw),
                Some("class" | "er")
            ) {
                assert!(
                    format_kzd(&src).is_err(),
                    "{name}: class/er fmt should report an explicit unsupported error"
                );
                // Still verify the parser round-trips these inputs.
                parse(&src).unwrap_or_else(|e| panic!("{name}: parse: {e:?}"));
                continue;
            }

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
        let src = "graph d { // opening comment\n  a: \"A\"\n  b: \"B\"\n  a -> b\n}";
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

    // --- M7a: State diagram tests ---

    #[test]
    fn parses_state_diagram_basic() {
        let src = r#"state traffic {
  state idle: "Idle"
  state active: "Active"
  [*] -> idle
  idle -> active : "start"
  active -> idle : "stop"
  active -> [*]
}"#;
        let d = parse(src).expect("should parse");
        let Diagram::State(s) = d else {
            panic!("expected State, got {:?}", d)
        };
        assert_eq!(s.states.len(), 2);
        assert_eq!(s.transitions.len(), 4);
        assert_eq!(s.states["idle"].label, "Idle");
    }

    #[test]
    fn state_auto_declaration() {
        let src = r#"state d {
  [*] -> foo
  foo -> bar
  bar -> [*]
}"#;
        let d = parse(src).expect("should parse");
        let Diagram::State(s) = d else { panic!() };
        assert!(s.states.contains_key("foo"));
        assert!(s.states.contains_key("bar"));
    }

    #[test]
    fn state_self_transition() {
        let src = r#"state d {
  state s: "S"
  [*] -> s
  s -> s : "loop"
}"#;
        let d = parse(src).expect("should parse");
        let Diagram::State(sd) = d else { panic!() };
        assert_eq!(sd.transitions.len(), 2);
        let self_t = sd.transitions.iter().find(|t| {
            matches!(&t.from, Endpoint::State(id) if id.as_str() == "s")
                && matches!(&t.to, Endpoint::State(id) if id.as_str() == "s")
        });
        assert!(self_t.is_some(), "self transition should be present");
    }

    #[test]
    fn state_with_explicit_label() {
        let src = r#"state d {
  state waiting: "Waiting for input"
  [*] -> waiting
}"#;
        let d = parse(src).expect("should parse");
        let Diagram::State(s) = d else { panic!() };
        assert_eq!(s.states["waiting"].label, "Waiting for input");
    }

    #[test]
    fn state_direction_is_error() {
        let src = r#"state d {
  state s: "S"
  direction down
  [*] -> s
}"#;
        let err = parse(src).expect_err("direction in state diagram should be error");
        assert!(
            err.iter().any(|e| e
                .message
                .contains("`direction` is not valid in state diagrams")),
            "got: {err:?}"
        );
    }

    #[test]
    fn state_dashed_edge_is_error() {
        let src = r#"state d {
  state a: "A"
  state b: "B"
  [*] -> a
  a --> b
}"#;
        let err = parse(src).expect_err("dashed edge in state diagram should be error");
        assert!(
            err.iter().any(|e| e.message.contains("dashed edges")),
            "got: {err:?}"
        );
    }

    #[test]
    fn state_dashed_pseudo_transition_is_explicit_error() {
        // Regression: `[*] --> s` / `s --> [*]` must yield the explicit
        // "dashed edges" diagnostic, not a generic syntax error.
        for src in ["state d {\n  [*] --> s\n}", "state d {\n  s --> [*]\n}"] {
            let err = parse(src).expect_err("dashed pseudo transition should error");
            assert!(
                err.iter().any(|e| e.message.contains("dashed edges")),
                "src {src:?} got: {err:?}"
            );
        }
    }

    #[test]
    fn pseudostate_transition_in_sequence_diagram_points_at_transition_not_whole_source() {
        // Regression: with keyword-based dispatch, a `[*]` pseudostate
        // transition inside a `sequence` diagram is rejected explicitly (no
        // signal-based inference), and the diagnostic must point at the
        // transition, not span the whole source.
        let src = "sequence d {\n  participant p\n  [*] -> s\n}";
        let err = parse(src).expect_err("[*] transition in a sequence diagram should error");
        let e = &err[0];
        assert!(
            e.message
                .contains("pseudostate transitions are only valid in state diagrams"),
            "got: {err:?}"
        );
        // The span must be a small slice (the `[*] -> s` transition), not the
        // entire document.
        assert!(
            e.span.end - e.span.start < src.chars().count(),
            "span should be narrow, got {:?}",
            e.span
        );
    }

    #[test]
    fn fmt_state_diagram_idempotent() {
        let src = r#"state traffic {
  state idle: "Idle"
  state active: "Active"

  [*] -> idle
  idle -> active : "start"
  active -> [*]
}"#;
        let formatted = format_kzd(src).expect("should format");
        let formatted2 = format_kzd(&formatted).expect("second format should succeed");
        assert_eq!(formatted, formatted2, "fmt must be idempotent");
    }

    #[test]
    fn fmt_state_semantic_preservation() {
        let src = r#"state traffic {
  state idle: "Idle"
  state active: "Active"

  [*] -> idle
  idle -> active : "start"
  active -> [*]
}"#;
        let formatted = format_kzd(src).expect("should format");
        let d1 = parse(src).expect("original should parse");
        let d2 = parse(&formatted).expect("formatted should parse");
        assert_eq!(d1, d2, "fmt must preserve state diagram semantics");
    }

    // -----------------------------------------------------------------------
    // Header keyword migration tests
    // -----------------------------------------------------------------------

    #[test]
    fn old_diagram_keyword_is_rejected() {
        let src = "diagram d { a\n b\n a -> b }";
        let err = parse(src).expect_err("`diagram` keyword should be rejected");
        assert!(
            err.iter().any(|e| e
                .message
                .contains("expected diagram kind keyword (graph|sequence|state|class|er)")),
            "got: {err:?}"
        );
    }

    #[test]
    fn unknown_kind_keyword_is_rejected() {
        let src = "flowchart d { a\n b\n a -> b }";
        let err = parse(src).expect_err("unknown kind keyword should be rejected");
        assert!(
            err.iter().any(|e| e
                .message
                .contains("expected diagram kind keyword (graph|sequence|state|class|er)")),
            "got: {err:?}"
        );
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
    fn class_basic_members_and_relations() {
        let src = r#"class orders {
  class Order {
    +id: Int
    +total: Money
    +submit(): void
  }
  interface Payable {
    +pay(): void
  }

  Customer "1" o-- "*" Order : "places"
  Dog --|> Animal
  Order ..|> Payable
}"#;
        let d = parse(src).expect("should parse");
        let c = class_diagram(&d);
        assert_eq!(c.classes["Order"].attributes[0], "+id: Int");
        assert_eq!(c.classes["Order"].methods[0], "+submit(): void");
        assert_eq!(
            c.classes["Payable"].stereotype.as_deref(),
            Some("interface")
        );
        assert_eq!(c.relations.len(), 3);
        let places = &c.relations[0];
        assert_eq!(places.from.as_str(), "Customer");
        assert_eq!(places.to.as_str(), "Order");
        assert_eq!(places.from_mult.as_deref(), Some("1"));
        assert_eq!(places.to_mult.as_deref(), Some("*"));
        assert_eq!(places.from_marker, kozue_ir::EndMarker::HollowDiamond);
        assert_eq!(places.label.as_deref(), Some("places"));
        let inherit = &c.relations[1];
        assert_eq!(inherit.to_marker, kozue_ir::EndMarker::HollowTriangle);
        assert_eq!(inherit.line, LineStyle::Solid);
        let realize = &c.relations[2];
        assert_eq!(realize.to_marker, kozue_ir::EndMarker::HollowTriangle);
        assert_eq!(realize.line, LineStyle::Dashed);
    }

    #[test]
    fn class_abstract_and_enum_stereotypes() {
        let src = "class d {\n  abstract class Shape\n  enum Color\n}";
        let d = parse(src).expect("should parse");
        let c = class_diagram(&d);
        assert_eq!(c.classes["Shape"].stereotype.as_deref(), Some("abstract"));
        assert_eq!(
            c.classes["Color"].stereotype.as_deref(),
            Some("enumeration")
        );
    }

    /// Helper: parse a single-relation class DSL body and return the relation.
    fn dsl_class_one_relation(rel_line: &str) -> kozue_ir::ClassRelation {
        let src = format!("class d {{\n  {rel_line}\n}}");
        let d = parse(&src).unwrap_or_else(|e| panic!("`{rel_line}` should parse: {e:?}"));
        let Diagram::Class(c) = d else {
            panic!("expected class diagram")
        };
        assert_eq!(c.relations.len(), 1, "`{rel_line}` produced != 1 relation");
        c.relations[0].clone()
    }

    #[test]
    fn class_all_connector_directions_are_accepted() {
        use kozue_ir::EndMarker::*;
        // Both spelling directions of every relation kind must parse, with the
        // marker on the end the glyph points at.
        let cases: &[(&str, kozue_ir::EndMarker, kozue_ir::EndMarker, LineStyle)] = &[
            ("A <|-- B", HollowTriangle, None, LineStyle::Solid),
            ("A --|> B", None, HollowTriangle, LineStyle::Solid),
            ("A <|.. B", HollowTriangle, None, LineStyle::Dashed),
            ("A ..|> B", None, HollowTriangle, LineStyle::Dashed),
            ("A *-- B", FilledDiamond, None, LineStyle::Solid),
            ("A --* B", None, FilledDiamond, LineStyle::Solid),
            ("A o-- B", HollowDiamond, None, LineStyle::Solid),
            ("A --o B", None, HollowDiamond, LineStyle::Solid),
            ("A --> B", None, OpenArrow, LineStyle::Solid),
            ("A <-- B", OpenArrow, None, LineStyle::Solid),
            ("A ..> B", None, OpenArrow, LineStyle::Dashed),
            ("A <.. B", OpenArrow, None, LineStyle::Dashed),
            ("A -- B", None, None, LineStyle::Solid),
            ("A .. B", None, None, LineStyle::Dashed),
        ];
        for &(line, from_m, to_m, ls) in cases {
            let r = dsl_class_one_relation(line);
            assert_eq!(r.from.as_str(), "A", "`{line}` from");
            assert_eq!(r.to.as_str(), "B", "`{line}` to");
            assert_eq!(r.from_marker, from_m, "`{line}` from_marker");
            assert_eq!(r.to_marker, to_m, "`{line}` to_marker");
            assert_eq!(r.line, ls, "`{line}` line");
        }
    }

    #[test]
    fn class_forward_and_reverse_tokens_are_mirror_images() {
        // The previously-rejected `<|--` and other reverse tokens must now be
        // accepted, and `A <op> B` must mirror `B <reverse-op> A`.
        for (fwd, rev) in [
            ("A <|-- B", "B --|> A"),
            ("A *-- B", "B --* A"),
            ("A o-- B", "B --o A"),
            ("A --> B", "B <-- A"),
            ("A ..|> B", "B <|.. A"),
            ("A ..> B", "B <.. A"),
        ] {
            let f = dsl_class_one_relation(fwd);
            let r = dsl_class_one_relation(rev);
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
    fn class_self_relation_is_error() {
        let src = "class d {\n  A --> A\n}";
        let err = parse(src).expect_err("self relation should be an error");
        assert!(
            err.iter().any(|e| e.message.contains("self relations")),
            "got: {err:?}"
        );
    }

    #[test]
    fn class_error_span_is_char_offset_not_byte_offset() {
        // class_dsl/er_dsl scan in byte offsets internally but must convert
        // to character offsets before returning, matching the rest of
        // kozue-dsl (chumsky) — wasm/lsp consumers treat all kozue_dsl spans
        // uniformly as character indices. Precede the erroring line with a
        // multi-byte comment so a leaked byte offset would misalign the span
        // against a `chars()`-indexed slice.
        let src = "class d {\n  // 日本語のコメント\n  this is not valid\n}";
        let err = parse(src).expect_err("should error");
        let span = err[0].span.clone();
        let chars: Vec<char> = src.chars().collect();
        assert!(span.end <= chars.len(), "span {span:?} out of char range");
        let text: String = chars[span].iter().collect();
        assert_eq!(
            text, "this is not valid",
            "char-indexed span must land exactly on the unrecognised text"
        );
    }

    #[test]
    fn class_unrecognised_statement_is_error() {
        let src = "class d {\n  this is not valid\n}";
        let err = parse(src).expect_err("should error");
        assert!(!err.is_empty());
    }

    #[test]
    fn class_unterminated_block_is_error() {
        let src = "class d {\n  class Order {\n    +id: Int\n}";
        let err = parse(src).expect_err("unterminated block should error");
        assert!(
            err.iter().any(|e| e.message.contains("unterminated")),
            "got: {err:?}"
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
    fn er_basic_entities_and_relation() {
        let src = r#"er shop {
  entity Customer {
    id: Int PK
    name: String
    email: String UK
  }
  entity Order {
    id: Int PK
  }

  Customer ||--o{ Order : "places"
}"#;
        let d = parse(src).expect("should parse");
        let e = er_diagram(&d);
        assert_eq!(e.entities.len(), 2);
        let customer = &e.entities["Customer"];
        assert_eq!(customer.attributes.len(), 3);
        assert_eq!(customer.attributes[0].keys, vec!["PK".to_string()]);
        assert_eq!(customer.attributes[2].keys, vec!["UK".to_string()]);
        let rel = &e.relations[0];
        assert_eq!(rel.from.as_str(), "Customer");
        assert_eq!(rel.to.as_str(), "Order");
        assert_eq!(rel.from_marker, kozue_ir::EndMarker::ErOne);
        assert_eq!(rel.to_marker, kozue_ir::EndMarker::ErZeroOrMany);
        assert_eq!(rel.label.as_deref(), Some("places"));
    }

    #[test]
    fn er_self_relation_is_error() {
        let src = "er d {\n  entity A { id: Int PK }\n  A ||--|| A : \"self\"\n}";
        let err = parse(src).expect_err("self relation should be an error");
        assert!(
            err.iter().any(|e| e.message.contains("self relations")),
            "got: {err:?}"
        );
    }

    #[test]
    fn er_unrecognised_statement_is_error() {
        let src = "er d {\n  this is not valid\n}";
        let err = parse(src).expect_err("should error");
        assert!(!err.is_empty());
    }

    #[test]
    fn er_fmt_style_comment_and_string_labels_do_not_break_parsing() {
        let src =
            "er d {\n  // top-level comment\n  entity A {\n    id: Int PK // primary key\n  }\n}";
        let d = parse(src).expect("comments should be stripped");
        let e = er_diagram(&d);
        assert_eq!(e.entities["A"].attributes[0].name, "id");
    }

    #[test]
    fn er_inline_single_line_entity_block() {
        // Spec example: `entity Order { id: Int PK; customer_id: Int FK }`.
        let src = "er shop {\n  entity Order { id: Int PK; customer_id: Int FK }\n}";
        let d = parse(src).expect("should parse");
        let e = er_diagram(&d);
        let order = &e.entities["Order"];
        assert_eq!(order.attributes.len(), 2);
        assert_eq!(order.attributes[0].name, "id");
        assert_eq!(order.attributes[0].keys, vec!["PK".to_string()]);
        assert_eq!(order.attributes[1].name, "customer_id");
        assert_eq!(order.attributes[1].keys, vec!["FK".to_string()]);
    }

    #[test]
    fn class_inline_single_line_interface_block() {
        // Spec example: `interface Payable { +pay(): void }`.
        let src = "class orders {\n  interface Payable { +pay(): void }\n}";
        let d = parse(src).expect("should parse");
        let c = class_diagram(&d);
        assert_eq!(
            c.classes["Payable"].stereotype.as_deref(),
            Some("interface")
        );
        assert_eq!(c.classes["Payable"].methods[0], "+pay(): void");
    }
}
