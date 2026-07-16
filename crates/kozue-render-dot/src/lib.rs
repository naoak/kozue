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
//!   Down / Right / Up / Left map to `rankdir=TB` / `LR` / `BT` / `RL`.
//! - [`Diagram::State`](kozue_ir::Diagram::State) — a `digraph` where named
//!   states are rounded boxes, the initial pseudostate is a filled `point`, and
//!   the final pseudostate is a `doublecircle`. Transitions become `->`
//!   statements.
//! - [`Diagram::Sequence`](kozue_ir::Diagram::Sequence) — **not supported.**
//!   DOT has no notion of lifelines or time ordering, so exporting a sequence
//!   diagram would silently discard its meaning. Returns
//!   [`RenderError::UnsupportedDiagram`] instead.
//! - [`Diagram::Class`](kozue_ir::Diagram::Class) — each class/interface
//!   becomes a Graphviz `record`-shaped node (`name` / `attributes` /
//!   `methods` fields); each relation becomes a `dir=both` edge whose
//!   `arrowtail`/`arrowhead` encode the UML end markers (see
//!   [`arrow_shape`]).
//! - [`Diagram::Er`](kozue_ir::Diagram::Er) — each entity becomes an
//!   HTML-like table node (`shape=plaintext`) with one row per attribute;
//!   each relation becomes a `dir=both` edge whose ends encode ER
//!   crow's-foot markers.

use kozue_ir::{
    ArrowType, ClassDiagram, ClassRelation, Diagram, Direction, EndMarker, Endpoint, ErDiagram,
    ErRelation, GraphDiagram, LineStyle, NodeKind, StateDiagram, Transition,
};

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
    /// A future IR direction has no defined Graphviz rank direction mapping.
    UnknownDirection {
        /// Debug description of the unresolved direction.
        direction: String,
    },
    /// A future graph node kind has no defined DOT mapping.
    UnknownNodeKind { kind: String },
    /// A future arrow type has no defined DOT mapping.
    UnknownArrowType { arrow: String },
    /// A future relation line style has no defined DOT mapping.
    UnknownLineStyle { line: String },
    /// A future relation end marker has no defined DOT mapping.
    UnknownEndMarker { marker: String },
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
            RenderError::UnknownDirection { direction } => {
                write!(f, "unsupported DOT rank direction: {direction}")
            }
            RenderError::UnknownNodeKind { kind } => {
                write!(f, "unsupported DOT graph node kind: {kind}")
            }
            RenderError::UnknownArrowType { arrow } => {
                write!(f, "unsupported DOT arrow type: {arrow}")
            }
            RenderError::UnknownLineStyle { line } => {
                write!(f, "unsupported DOT line style: {line}")
            }
            RenderError::UnknownEndMarker { marker } => {
                write!(f, "unsupported DOT end marker: {marker}")
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
        Diagram::Class(c) => render_class(c),
        Diagram::Er(e) => render_er(e),
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
    let rankdir = rankdir(g.direction)?;
    out.push_str(&format!("  rankdir={rankdir};\n"));
    out.push_str("  node [shape=box style=rounded];\n");

    // Node statements, in declaration order.
    for node in g.nodes.values() {
        let shape_attrs = match &node.kind {
            NodeKind::Default => "",
            NodeKind::Rectangle => " shape=box style=\"\"",
            NodeKind::RoundedRectangle => " shape=box style=rounded",
            NodeKind::Circle => " shape=circle style=\"\"",
            NodeKind::Diamond => " shape=diamond style=\"\"",
            kind => {
                return Err(RenderError::UnknownNodeKind {
                    kind: format!("{kind:?}"),
                })
            }
        };
        out.push_str(&format!(
            "  {} [label={}{}];\n",
            quote(node.id.as_str()),
            quote(&node.label),
            shape_attrs,
        ));
    }

    // Edge statements, in declaration order.
    for edge in &g.edges {
        if !g.nodes.contains_key(&edge.from) {
            return Err(RenderError::DanglingEdge {
                node_id: edge.from.to_string(),
            });
        }
        if !g.nodes.contains_key(&edge.to) {
            return Err(RenderError::DanglingEdge {
                node_id: edge.to.to_string(),
            });
        }
        out.push_str(&edge_stmt(
            edge.from.as_str(),
            edge.to.as_str(),
            edge.label.as_deref(),
            edge.arrow,
        )?);
    }

    out.push_str("}\n");
    Ok(out)
}

fn rankdir(direction: Direction) -> Result<&'static str, RenderError> {
    let rankdir = match direction {
        Direction::Down => "TB",
        Direction::Right => "LR",
        Direction::Up => "BT",
        Direction::Left => "RL",
        _ => {
            return Err(RenderError::UnknownDirection {
                direction: format!("{direction:?}"),
            })
        }
    };
    Ok(rankdir)
}

/// Format a single `a -> b [attrs];` statement.
fn edge_stmt(
    from: &str,
    to: &str,
    label: Option<&str>,
    arrow: ArrowType,
) -> Result<String, RenderError> {
    let mut attrs: Vec<String> = Vec::new();
    if let Some(l) = label {
        attrs.push(format!("label={}", quote(l)));
    }
    match arrow {
        ArrowType::Triangle => {}
        ArrowType::None => attrs.push("dir=none".to_string()),
        _ => {
            return Err(RenderError::UnknownArrowType {
                arrow: format!("{arrow:?}"),
            })
        }
    }
    if attrs.is_empty() {
        Ok(format!("  {} -> {};\n", quote(from), quote(to)))
    } else {
        Ok(format!(
            "  {} -> {} [{}];\n",
            quote(from),
            quote(to),
            attrs.join(" ")
        ))
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
            quote(state.id.as_str()),
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
    edge_stmt(&from, &to, t.label.as_deref(), ArrowType::Triangle)
}

/// Resolve a transition endpoint to a DOT node identifier.
fn endpoint_id(s: &StateDiagram, ep: &Endpoint) -> Result<String, RenderError> {
    match ep {
        Endpoint::Initial => Ok(INITIAL_ID.to_string()),
        Endpoint::Final => Ok(FINAL_ID.to_string()),
        Endpoint::State(id) => {
            if s.states.contains_key(id) {
                Ok(id.to_string())
            } else {
                Err(RenderError::DanglingEdge {
                    node_id: id.to_string(),
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
// Class diagram
// ---------------------------------------------------------------------------

fn render_class(c: &ClassDiagram) -> Result<String, RenderError> {
    let mut out = String::new();
    out.push_str("digraph {\n");
    let rankdir = rankdir(c.direction)?;
    out.push_str(&format!("  rankdir={rankdir};\n"));
    out.push_str("  node [shape=record];\n");

    for node in c.classes.values() {
        out.push_str(&format!(
            "  {} [label=\"{}\"];\n",
            quote(node.id.as_str()),
            class_record_label(node)
        ));
    }

    for rel in &c.relations {
        if !c.classes.contains_key(&rel.from) {
            return Err(RenderError::DanglingEdge {
                node_id: rel.from.to_string(),
            });
        }
        if !c.classes.contains_key(&rel.to) {
            return Err(RenderError::DanglingEdge {
                node_id: rel.to.to_string(),
            });
        }
        out.push_str(&class_relation_stmt(rel)?);
    }

    out.push_str("}\n");
    Ok(out)
}

/// Build the (unquoted, un-outer-wrapped) Graphviz record-label content for a
/// class node: `{<<stereotype>>\nName|attr1\lattr2\l|method1()\lmethod2()\l}`.
/// Empty attribute/method sections are omitted as record fields entirely.
fn class_record_label(node: &kozue_ir::ClassNode) -> String {
    let title = match &node.stereotype {
        Some(st) => format!(
            "\u{ab}{}\u{bb}\\n{}",
            record_escape(st),
            record_escape(&node.name)
        ),
        None => record_escape(&node.name),
    };
    let mut fields = vec![title];
    if !node.attributes.is_empty() {
        fields.push(record_rows(&node.attributes));
    }
    if !node.methods.is_empty() {
        fields.push(record_rows(&node.methods));
    }
    format!("{{{}}}", fields.join("|"))
}

/// Join display rows into one left-justified Graphviz record field
/// (`row1\lrow2\l`, trailing `\l` included after the last row too).
fn record_rows(rows: &[String]) -> String {
    let mut s = String::new();
    for row in rows {
        s.push_str(&record_escape(row));
        s.push_str("\\l");
    }
    s
}

/// Escape characters that are structurally significant inside a Graphviz
/// record label (`{ } | < >`), backslash, and double quote, so
/// user-controlled text (class/attribute/method names) can never break out
/// of the record field it appears in or the surrounding quoted DOT string.
/// Unlike [`quote`] (used for plain labels), this does **not** double
/// backslashes: Graphviz's own lexer only special-cases `\"` inside a quoted
/// string and otherwise passes `\` through unchanged, so a single backslash
/// here is what the record parser needs to see.
fn record_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '|' => out.push_str("\\|"),
            '<' => out.push_str("\\<"),
            '>' => out.push_str("\\>"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            _ => out.push(ch),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// ER diagram
// ---------------------------------------------------------------------------

fn render_er(e: &ErDiagram) -> Result<String, RenderError> {
    let mut out = String::new();
    out.push_str("digraph {\n");
    out.push_str("  rankdir=TB;\n");
    out.push_str("  node [shape=plaintext];\n");

    for entity in e.entities.values() {
        out.push_str(&format!(
            "  {} [label=<{}>];\n",
            quote(entity.id.as_str()),
            er_table_label(entity)
        ));
    }

    for rel in &e.relations {
        if !e.entities.contains_key(&rel.from) {
            return Err(RenderError::DanglingEdge {
                node_id: rel.from.to_string(),
            });
        }
        if !e.entities.contains_key(&rel.to) {
            return Err(RenderError::DanglingEdge {
                node_id: rel.to.to_string(),
            });
        }
        out.push_str(&er_relation_stmt(rel)?);
    }

    out.push_str("}\n");
    Ok(out)
}

/// Build a Graphviz HTML-like `<TABLE>` label for an ER entity: a header row
/// with the entity name, followed by one row per attribute (keys / type /
/// name / comment).
fn er_table_label(entity: &kozue_ir::ErEntity) -> String {
    let mut s =
        String::from("<TABLE BORDER=\"0\" CELLBORDER=\"1\" CELLSPACING=\"0\" CELLPADDING=\"4\">");
    s.push_str(&format!(
        "<TR><TD COLSPAN=\"4\" BGCOLOR=\"LIGHTGREY\"><B>{}</B></TD></TR>",
        html_escape(&entity.name)
    ));
    for attr in &entity.attributes {
        let keys = attr.keys.join(",");
        let comment = attr.comment.as_deref().unwrap_or("");
        s.push_str(&format!(
            "<TR><TD>{}</TD><TD ALIGN=\"LEFT\">{}</TD><TD ALIGN=\"LEFT\">{}</TD><TD ALIGN=\"LEFT\">{}</TD></TR>",
            html_escape(&keys),
            html_escape(&attr.type_name),
            html_escape(&attr.name),
            html_escape(comment),
        ));
    }
    s.push_str("</TABLE>");
    s
}

/// Escape `& < > "` for use inside a Graphviz HTML-like label (delimited by
/// bare `<` `>`, not a quoted DOT string — a different escaping context than
/// [`quote`]/[`record_escape`]).
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
    out
}

fn er_relation_stmt(rel: &ErRelation) -> Result<String, RenderError> {
    relation_stmt(
        rel.from.as_str(),
        rel.to.as_str(),
        rel.from_marker,
        rel.to_marker,
        rel.line,
        rel.label.as_deref(),
        None,
        None,
    )
}

// ---------------------------------------------------------------------------
// Shared relation/marker rendering (class + ER)
// ---------------------------------------------------------------------------

fn class_relation_stmt(rel: &ClassRelation) -> Result<String, RenderError> {
    relation_stmt(
        rel.from.as_str(),
        rel.to.as_str(),
        rel.from_marker,
        rel.to_marker,
        rel.line,
        rel.label.as_deref(),
        rel.from_mult.as_deref(),
        rel.to_mult.as_deref(),
    )
}

/// Format a `from -> to [dir=both arrowtail=... arrowhead=... ...];`
/// statement shared by class relations and ER relations. `from_marker`
/// draws at the `from` end (`arrowtail`), `to_marker` at the `to` end
/// (`arrowhead`); `dir=both` is required for `arrowtail` to have any
/// effect in Graphviz.
#[allow(clippy::too_many_arguments)]
fn relation_stmt(
    from: &str,
    to: &str,
    from_marker: EndMarker,
    to_marker: EndMarker,
    line: LineStyle,
    label: Option<&str>,
    from_mult: Option<&str>,
    to_mult: Option<&str>,
) -> Result<String, RenderError> {
    let mut attrs = vec![
        "dir=both".to_string(),
        format!("arrowtail={}", arrow_shape(from_marker)?),
        format!("arrowhead={}", arrow_shape(to_marker)?),
    ];
    match line {
        LineStyle::Solid => {}
        LineStyle::Dashed => attrs.push("style=dashed".to_string()),
        _ => {
            return Err(RenderError::UnknownLineStyle {
                line: format!("{line:?}"),
            })
        }
    }
    if let Some(l) = label {
        attrs.push(format!("label={}", quote(l)));
    }
    if let Some(m) = from_mult {
        attrs.push(format!("taillabel={}", quote(m)));
    }
    if let Some(m) = to_mult {
        attrs.push(format!("headlabel={}", quote(m)));
    }
    Ok(format!(
        "  {} -> {} [{}];\n",
        quote(from),
        quote(to),
        attrs.join(" ")
    ))
}

/// Map an [`EndMarker`] to a Graphviz arrow-shape name.
///
/// | `EndMarker`       | Graphviz shape |
/// |--------------------|----------------|
/// | `None`             | `none`         |
/// | `HollowTriangle`    | `empty`        |
/// | `OpenArrow`         | `vee`          |
/// | `FilledDiamond`     | `diamond`      |
/// | `HollowDiamond`     | `odiamond`     |
/// | `ErOne`             | `tee`          |
/// | `ErMany`            | `crow`         |
/// | `ErZeroOrOne`       | `odottee`      |
/// | `ErOneOrMany`       | `teecrow`      |
/// | `ErZeroOrMany`      | `odotcrow`     |
///
/// Future `#[non_exhaustive]` variants are explicit errors.
fn arrow_shape(marker: EndMarker) -> Result<&'static str, RenderError> {
    Ok(match marker {
        EndMarker::None => "none",
        EndMarker::HollowTriangle => "empty",
        EndMarker::OpenArrow => "vee",
        EndMarker::FilledDiamond => "diamond",
        EndMarker::HollowDiamond => "odiamond",
        EndMarker::ErOne => "tee",
        EndMarker::ErMany => "crow",
        EndMarker::ErZeroOrOne => "odottee",
        EndMarker::ErOneOrMany => "teecrow",
        EndMarker::ErZeroOrMany => "odotcrow",
        _ => {
            return Err(RenderError::UnknownEndMarker {
                marker: format!("{marker:?}"),
            })
        }
    })
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
    use kozue_ir::{ClassNode, Edge, ErAttribute, ErEntity, Node, State, Transition};

    #[test]
    fn known_arrow_line_and_marker_mappings_are_preserved() {
        assert_eq!(
            edge_stmt("a", "b", None, ArrowType::Triangle).unwrap(),
            "  \"a\" -> \"b\";\n"
        );
        assert!(edge_stmt("a", "b", None, ArrowType::None)
            .unwrap()
            .contains("dir=none"));
        for (marker, expected) in [
            (EndMarker::None, "none"),
            (EndMarker::HollowTriangle, "empty"),
            (EndMarker::OpenArrow, "vee"),
            (EndMarker::FilledDiamond, "diamond"),
            (EndMarker::HollowDiamond, "odiamond"),
            (EndMarker::ErOne, "tee"),
            (EndMarker::ErMany, "crow"),
            (EndMarker::ErZeroOrOne, "odottee"),
            (EndMarker::ErOneOrMany, "teecrow"),
            (EndMarker::ErZeroOrMany, "odotcrow"),
        ] {
            assert_eq!(arrow_shape(marker).unwrap(), expected);
        }
        assert!(relation_stmt(
            "a",
            "b",
            EndMarker::None,
            EndMarker::None,
            LineStyle::Dashed,
            None,
            None,
            None,
        )
        .unwrap()
        .contains("style=dashed"));
    }

    fn sample_class_diagram() -> ClassDiagram {
        let mut cd = ClassDiagram::new(Direction::Down);

        let mut animal = ClassNode::new("Animal", "Animal");
        animal.stereotype = Some("abstract".to_string());
        animal.attributes.push("+name: String".to_string());
        animal.methods.push("+speak(): void".to_string());
        cd.classes.insert("Animal".into(), animal);

        let mut dog = ClassNode::new("Dog", "Dog");
        dog.methods.push("+bark(): void".to_string());
        cd.classes.insert("Dog".into(), dog);

        let mut kennel = ClassNode::new("Kennel", "Kennel");
        kennel.attributes.push("+capacity: Int".to_string());
        cd.classes.insert("Kennel".into(), kennel);

        // Dog --|> Animal (inheritance: hollow triangle at the `to` end).
        cd.relations.push(ClassRelation::new(
            "Dog",
            "Animal",
            EndMarker::None,
            EndMarker::HollowTriangle,
            LineStyle::Solid,
            None,
            None,
            None,
        ));
        // Kennel *-- Dog (composition: filled diamond at the `from` end).
        cd.relations.push(ClassRelation::new(
            "Kennel",
            "Dog",
            EndMarker::FilledDiamond,
            EndMarker::None,
            LineStyle::Dashed,
            Some("houses".to_string()),
            Some("1".to_string()),
            Some("*".to_string()),
        ));
        cd
    }

    fn sample_er_diagram() -> ErDiagram {
        let mut ed = ErDiagram::new();

        let mut customer = ErEntity::new("Customer", "CUSTOMER");
        customer
            .attributes
            .push(ErAttribute::new("int", "id", vec!["PK".to_string()], None));
        ed.entities.insert("Customer".into(), customer);

        let mut order = ErEntity::new("Order", "ORDER");
        order.attributes.push(ErAttribute::new(
            "int",
            "customer_id",
            vec!["FK".to_string()],
            Some("references Customer".to_string()),
        ));
        ed.entities.insert("Order".into(), order);

        // CUSTOMER ||--o{ ORDER : places
        ed.relations.push(ErRelation::new(
            "Customer",
            "Order",
            EndMarker::ErOne,
            EndMarker::ErZeroOrMany,
            Some("places".to_string()),
            LineStyle::Solid,
        ));
        ed
    }

    #[test]
    fn class_render_uses_record_shape_and_markers() {
        let dot = render(&Diagram::Class(sample_class_diagram())).expect("class render");
        assert!(
            dot.contains("shape=record"),
            "class nodes are records: {dot}"
        );
        assert!(dot.contains("Animal"), "class name must appear: {dot}");
        assert!(
            dot.contains("+speak(): void"),
            "method row must appear: {dot}"
        );
        assert!(
            dot.contains("arrowhead=empty"),
            "inheritance uses a hollow triangle head: {dot}"
        );
        assert!(
            dot.contains("arrowtail=diamond"),
            "composition uses a filled diamond tail: {dot}"
        );
        assert!(dot.contains("style=dashed"), "dashed relation: {dot}");
        assert!(
            dot.contains("taillabel=\"1\"") && dot.contains("headlabel=\"*\""),
            "multiplicities must appear: {dot}"
        );
        assert!(dot.contains("dir=both"), "both ends must be styled: {dot}");
    }

    #[test]
    fn class_render_does_not_fall_back_to_unsupported() {
        let err = render(&Diagram::Class(sample_class_diagram()));
        assert!(err.is_ok());
    }

    #[test]
    fn class_render_dangling_relation_is_error() {
        let mut cd = sample_class_diagram();
        cd.relations.push(ClassRelation::new(
            "Dog",
            "Ghost",
            EndMarker::None,
            EndMarker::None,
            LineStyle::Solid,
            None,
            None,
            None,
        ));
        let err = render(&Diagram::Class(cd)).unwrap_err();
        assert_eq!(
            err,
            RenderError::DanglingEdge {
                node_id: "Ghost".into()
            }
        );
    }

    #[test]
    fn class_render_is_deterministic() {
        let d = Diagram::Class(sample_class_diagram());
        assert_eq!(render(&d).unwrap(), render(&d).unwrap());
    }

    #[test]
    fn er_render_uses_html_table_and_crowsfoot_markers() {
        let dot = render(&Diagram::Er(sample_er_diagram())).expect("er render");
        assert!(dot.contains("shape=plaintext"), "er nodes: {dot}");
        assert!(dot.contains("<TABLE"), "html table label: {dot}");
        assert!(dot.contains("CUSTOMER"), "entity name must appear: {dot}");
        assert!(
            dot.contains("customer_id"),
            "attribute row must appear: {dot}"
        );
        assert!(dot.contains("arrowtail=tee"), "ErOne maps to tee: {dot}");
        assert!(
            dot.contains("arrowhead=odotcrow"),
            "ErZeroOrMany maps to odotcrow: {dot}"
        );
        assert!(dot.contains("label=\"places\""), "relation label: {dot}");
    }

    #[test]
    fn er_render_dangling_relation_is_error() {
        let mut ed = sample_er_diagram();
        ed.relations.push(ErRelation::new(
            "Customer",
            "Ghost",
            EndMarker::None,
            EndMarker::None,
            None,
            LineStyle::Solid,
        ));
        let err = render(&Diagram::Er(ed)).unwrap_err();
        assert_eq!(
            err,
            RenderError::DanglingEdge {
                node_id: "Ghost".into()
            }
        );
    }

    #[test]
    fn er_render_is_deterministic() {
        let d = Diagram::Er(sample_er_diagram());
        assert_eq!(render(&d).unwrap(), render(&d).unwrap());
    }

    #[test]
    fn record_escape_escapes_structural_chars() {
        assert_eq!(record_escape("a{b}c|d<e>f"), "a\\{b\\}c\\|d\\<e\\>f");
    }

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
    fn graph_and_class_map_all_four_rank_directions() {
        for (direction, rankdir) in [
            (Direction::Down, "TB"),
            (Direction::Right, "LR"),
            (Direction::Up, "BT"),
            (Direction::Left, "RL"),
        ] {
            let graph_dot = render(&Diagram::Graph(graph(direction))).unwrap();
            assert!(graph_dot.contains(&format!("  rankdir={rankdir};\n")));

            let mut class = sample_class_diagram();
            class.direction = direction;
            let class_dot = render(&Diagram::Class(class)).unwrap();
            assert!(class_dot.contains(&format!("  rankdir={rankdir};\n")));
        }
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
        s.transitions.push(Transition::new(
            Endpoint::Initial,
            Endpoint::State("idle".into()),
            None,
        ));
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
