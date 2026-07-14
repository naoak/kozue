//! Deterministic Graphviz DOT exporter for the semantic IR.
//!
//! ## Rationale
//!
//! DOT is the native input language of Graphviz. Unlike every other backend in
//! kozue, DOT is not a rendered *picture* — it is a *graph description* that
//! Graphviz (or any DOT-consuming tool) lays out itself. That makes the
//! exporter deliberately lightweight: it reads the **semantic**
//! [`Diagram`](kozue_ir::Diagram) directly and never touches the kozue layout
//! engine or the [`Scene`](kozue_ir::Scene). Nodes, edges, labels and arrow
//! directions map almost one-to-one onto DOT statements.
//!
//! ## Determinism
//!
//! Output is byte-identical for the same input:
//! - No `HashMap` anywhere; nodes come from an [`IndexMap`](indexmap::IndexMap)
//!   (declaration order) and edges from a `Vec`.
//! - Node identifiers are the original diagram IDs, always double-quoted and
//!   escaped, so ordering and escaping are fixed.
//! - Statement order is: all node statements (in declaration order) followed by
//!   all edge statements (in declaration order).
//! - Indentation (two spaces) and attribute order are fixed.
//!
//! ## Supported diagram types
//!
//! - [`Diagram::Graph`](kozue_ir::Diagram::Graph) — a `digraph`. Each node
//!   becomes a rounded-box statement; each edge becomes a `->` statement.
//!   [`Direction::Right`](kozue_ir::Direction::Right) maps to `rankdir=LR`,
//!   [`Direction::Down`](kozue_ir::Direction::Down) to `rankdir=TB`.
//! - [`Diagram::State`](kozue_ir::Diagram::State) — a `digraph` where named
//!   states are rounded boxes, the initial pseudostate is a filled `point`, and
//!   the final pseudostate is a `doublecircle`. Transitions become `->`
//!   statements.
//! - [`Diagram::Sequence`](kozue_ir::Diagram::Sequence) — **not supported.**
//!   DOT has no notion of lifelines or time ordering, so exporting a sequence
//!   diagram would silently discard its meaning. Returns
//!   [`RenderError::UnsupportedDiagram`] instead.

use kozue_ir::{ArrowType, Diagram, Direction, Endpoint, GraphDiagram, StateDiagram, Transition};

// ---------------------------------------------------------------------------
// Public error type
// ---------------------------------------------------------------------------

/// An error that prevents a [`Diagram`] from being exported to DOT.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderError {
    /// The diagram type has no faithful DOT representation (e.g. sequence
    /// diagrams). Returns an explicit error instead of silently dropping data.
    UnsupportedDiagram {
        /// Human-readable description of the unsupported variant.
        kind: &'static str,
    },
    /// An edge or transition references a node/state ID that is not declared.
    /// Emitting the edge anyway would introduce an implicit node in Graphviz,
    /// changing the graph; surfacing an error is safer.
    DanglingEdge {
        /// The missing node/state ID.
        node_id: String,
    },
    /// A state transition references an endpoint that cannot be resolved (a
    /// future `#[non_exhaustive]` [`Endpoint`] variant).
    UnknownEndpoint {
        /// Debug description of the unresolved endpoint.
        endpoint: String,
    },
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::UnsupportedDiagram { kind } => {
                write!(f, "DOT export does not support {kind} diagrams")
            }
            RenderError::DanglingEdge { node_id } => {
                write!(f, "edge references undeclared node `{node_id}`")
            }
            RenderError::UnknownEndpoint { endpoint } => {
                write!(f, "unresolved transition endpoint: {endpoint}")
            }
        }
    }
}

impl std::error::Error for RenderError {}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Render a semantic [`Diagram`] to a Graphviz DOT document.
pub fn render(diagram: &Diagram) -> Result<String, RenderError> {
    match diagram {
        Diagram::Graph(g) => render_graph(g),
        Diagram::State(s) => render_state(s),
        Diagram::Sequence(_) => Err(RenderError::UnsupportedDiagram { kind: "sequence" }),
        // `Diagram` is `#[non_exhaustive]`; refuse unknown variants rather than
        // emitting an empty graph.
        _ => Err(RenderError::UnsupportedDiagram { kind: "unknown" }),
    }
}

// ---------------------------------------------------------------------------
// Graph
// ---------------------------------------------------------------------------

fn render_graph(g: &GraphDiagram) -> Result<String, RenderError> {
    let mut out = String::new();
    out.push_str("digraph {\n");
    let rankdir = match g.direction {
        Direction::Right => "LR",
        // `Direction` is `#[non_exhaustive]`; `Down` and any future top-down
        // variant map to Graphviz's default top-to-bottom ranking.
        _ => "TB",
    };
    out.push_str(&format!("  rankdir={rankdir};\n"));
    out.push_str("  node [shape=box style=rounded];\n");

    // Node statements, in declaration order.
    for node in g.nodes.values() {
        out.push_str(&format!(
            "  {} [label={}];\n",
            quote(&node.id),
            quote(&node.label)
        ));
    }

    // Edge statements, in declaration order.
    for edge in &g.edges {
        if !g.nodes.contains_key(&edge.from) {
            return Err(RenderError::DanglingEdge {
                node_id: edge.from.clone(),
            });
        }
        if !g.nodes.contains_key(&edge.to) {
            return Err(RenderError::DanglingEdge {
                node_id: edge.to.clone(),
            });
        }
        out.push_str(&edge_stmt(&edge.from, &edge.to, edge.label.as_deref(), edge.arrow));
    }

    out.push_str("}\n");
    Ok(out)
}

/// Format a single `a -> b [attrs];` statement.
fn edge_stmt(from: &str, to: &str, label: Option<&str>, arrow: ArrowType) -> String {
    let mut attrs: Vec<String> = Vec::new();
    if let Some(l) = label {
        attrs.push(format!("label={}", quote(l)));
    }
    if arrow == ArrowType::None {
        attrs.push("dir=none".to_string());
    }
    if attrs.is_empty() {
        format!("  {} -> {};\n", quote(from), quote(to))
    } else {
        format!("  {} -> {} [{}];\n", quote(from), quote(to), attrs.join(" "))
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Fixed node IDs for the pseudostates. State IDs are user-controlled, so a
/// collision is possible in principle; the leading underscore keeps them out of
/// the way of typical identifiers, and the IDs are quoted regardless.
const INITIAL_ID: &str = "__initial__";
const FINAL_ID: &str = "__final__";

fn render_state(s: &StateDiagram) -> Result<String, RenderError> {
    let mut out = String::new();
    out.push_str("digraph {\n");
    out.push_str("  rankdir=TB;\n");
    out.push_str("  node [shape=box style=rounded];\n");

    // Pseudostates are only emitted when a transition actually references them.
    let uses_initial = s
        .transitions
        .iter()
        .any(|t| matches!(t.from, Endpoint::Initial) || matches!(t.to, Endpoint::Initial));
    let uses_final = s
        .transitions
        .iter()
        .any(|t| matches!(t.from, Endpoint::Final) || matches!(t.to, Endpoint::Final));
    if uses_initial {
        out.push_str(&format!(
            "  {} [shape=point width=0.2];\n",
            quote(INITIAL_ID)
        ));
    }
    if uses_final {
        out.push_str(&format!(
            "  {} [shape=doublecircle label=\"\" width=0.2];\n",
            quote(FINAL_ID)
        ));
    }

    // State node statements, in declaration order.
    for state in s.states.values() {
        out.push_str(&format!(
            "  {} [label={}];\n",
            quote(&state.id),
            quote(&state.label)
        ));
    }

    // Transition statements, in declaration order.
    for t in &s.transitions {
        out.push_str(&transition_stmt(s, t)?);
    }

    out.push_str("}\n");
    Ok(out)
}

fn transition_stmt(s: &StateDiagram, t: &Transition) -> Result<String, RenderError> {
    let from = endpoint_id(s, &t.from)?;
    let to = endpoint_id(s, &t.to)?;
    Ok(edge_stmt(&from, &to, t.label.as_deref(), ArrowType::Triangle))
}

/// Resolve a transition endpoint to a DOT node identifier.
fn endpoint_id(s: &StateDiagram, ep: &Endpoint) -> Result<String, RenderError> {
    match ep {
        Endpoint::Initial => Ok(INITIAL_ID.to_string()),
        Endpoint::Final => Ok(FINAL_ID.to_string()),
        Endpoint::State(id) => {
            if s.states.contains_key(id) {
                Ok(id.clone())
            } else {
                Err(RenderError::DanglingEdge {
                    node_id: id.clone(),
                })
            }
        }
        // `Endpoint` is `#[non_exhaustive]`; a future variant we can't resolve
        // is surfaced rather than silently dropped.
        other => Err(RenderError::UnknownEndpoint {
            endpoint: format!("{other:?}"),
        }),
    }
}

// ---------------------------------------------------------------------------
// Escaping
// ---------------------------------------------------------------------------

/// Wrap a string in double quotes, escaping the characters that are special
/// inside a DOT double-quoted ID: backslash, double quote, and newline.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use kozue_ir::{Edge, Node, State, Transition};

    fn graph(direction: Direction) -> GraphDiagram {
        GraphDiagram::new(direction)
    }

    #[test]
    fn graph_renders_nodes_and_edges() {
        let mut g = graph(Direction::Down);
        g.nodes.insert("a".into(), Node::new("a", "Start"));
        g.nodes.insert("b".into(), Node::new("b", "End"));
        g.edges
            .push(Edge::new("a", "b", Some("go".into()), ArrowType::Triangle));
        let dot = render(&Diagram::Graph(g)).unwrap();
        assert!(dot.starts_with("digraph {\n"));
        assert!(dot.contains("  rankdir=TB;\n"));
        assert!(dot.contains("  \"a\" [label=\"Start\"];\n"));
        assert!(dot.contains("  \"a\" -> \"b\" [label=\"go\"];\n"));
        assert!(dot.ends_with("}\n"));
    }

    #[test]
    fn right_direction_maps_to_lr() {
        let g = graph(Direction::Right);
        let dot = render(&Diagram::Graph(g)).unwrap();
        assert!(dot.contains("  rankdir=LR;\n"));
    }

    #[test]
    fn none_arrow_emits_dir_none() {
        let mut g = graph(Direction::Down);
        g.nodes.insert("a".into(), Node::new("a", "A"));
        g.nodes.insert("b".into(), Node::new("b", "B"));
        g.edges.push(Edge::new("a", "b", None, ArrowType::None));
        let dot = render(&Diagram::Graph(g)).unwrap();
        assert!(dot.contains("  \"a\" -> \"b\" [dir=none];\n"));
    }

    #[test]
    fn dangling_edge_is_error() {
        let mut g = graph(Direction::Down);
        g.nodes.insert("a".into(), Node::new("a", "A"));
        g.edges
            .push(Edge::new("a", "ghost", None, ArrowType::Triangle));
        let err = render(&Diagram::Graph(g)).unwrap_err();
        assert_eq!(
            err,
            RenderError::DanglingEdge {
                node_id: "ghost".into()
            }
        );
    }

    #[test]
    fn state_emits_pseudostates_only_when_used() {
        let mut s = StateDiagram::new();
        s.states.insert("idle".into(), State::new("idle", "Idle"));
        s.transitions
            .push(Transition::new(Endpoint::Initial, Endpoint::State("idle".into()), None));
        s.transitions.push(Transition::new(
            Endpoint::State("idle".into()),
            Endpoint::Final,
            None,
        ));
        let dot = render(&Diagram::State(s)).unwrap();
        assert!(dot.contains(&format!("  {} [shape=point", quote(INITIAL_ID))));
        assert!(dot.contains(&format!("  {} [shape=doublecircle", quote(FINAL_ID))));
        assert!(dot.contains(&format!("  {} -> \"idle\";\n", quote(INITIAL_ID))));
    }

    #[test]
    fn sequence_is_unsupported() {
        let seq = kozue_ir::SequenceDiagram::new();
        let err = render(&Diagram::Sequence(seq)).unwrap_err();
        assert_eq!(err, RenderError::UnsupportedDiagram { kind: "sequence" });
    }

    #[test]
    fn quoting_escapes_specials() {
        assert_eq!(quote(r#"a"b\c"#), r#""a\"b\\c""#);
    }

    #[test]
    fn render_is_deterministic() {
        let mut g = graph(Direction::Down);
        g.nodes.insert("a".into(), Node::new("a", "A"));
        g.nodes.insert("b".into(), Node::new("b", "B"));
        g.edges
            .push(Edge::new("a", "b", Some("x".into()), ArrowType::Triangle));
        let d = Diagram::Graph(g);
        assert_eq!(render(&d).unwrap(), render(&d).unwrap());
    }
}
