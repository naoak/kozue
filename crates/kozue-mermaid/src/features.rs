//! Mermaid compatibility feature table.
//!
//! This is the single source of truth for what kozue-mermaid supports. It is
//! used by `kozue compat mermaid` to display a table, and can be used in the
//! future to generate documentation or drive test parameterisation.

/// Support level of a Mermaid feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    /// Fully supported; output matches expected semantics.
    Supported,
    /// Partially supported; some aspects may differ from Mermaid.
    Partial,
    /// Not supported; produces an "unsupported" error.
    Unsupported,
}

impl Support {
    pub fn as_str(self) -> &'static str {
        match self {
            Support::Supported => "supported",
            Support::Partial => "partial",
            Support::Unsupported => "unsupported",
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Support::Supported => "✓",
            Support::Partial => "~",
            Support::Unsupported => "✗",
        }
    }
}

/// A single Mermaid feature entry in the compatibility table.
#[derive(Debug, Clone)]
pub struct Feature {
    /// Short human-readable feature name.
    pub name: &'static str,
    /// Support level.
    pub support: Support,
    /// A note explaining the support level or any caveats.
    pub note: &'static str,
}

/// The full Mermaid compatibility table.
///
/// One canonical list — display, docs, and tests all derive from this.
pub const FEATURES: &[Feature] = &[
    // --- Diagram types ---
    Feature {
        name: "flowchart / graph",
        support: Support::Supported,
        note: "flowchart TD/TB/LR; `graph` keyword also accepted",
    },
    Feature {
        name: "sequenceDiagram",
        support: Support::Supported,
        note: "participant declarations, ->> and -->> messages, self-messages",
    },
    Feature {
        name: "stateDiagram-v2 / stateDiagram",
        support: Support::Supported,
        note: "[*] pseudostates, --> transitions with labels, `state \"desc\" as id`, auto-declared states",
    },
    // --- Flowchart directions ---
    Feature {
        name: "direction TD / TB",
        support: Support::Supported,
        note: "both map to Direction::Down",
    },
    Feature {
        name: "direction LR",
        support: Support::Supported,
        note: "maps to Direction::Right",
    },
    Feature {
        name: "direction RL",
        support: Support::Unsupported,
        note: "reports an unsupported error; kozue layout does not support right-to-left",
    },
    Feature {
        name: "direction BT",
        support: Support::Unsupported,
        note: "reports an unsupported error; kozue layout does not support bottom-to-top",
    },
    // --- Flowchart nodes ---
    Feature {
        name: "rectangular node A[label]",
        support: Support::Supported,
        note: "maps to NodeKind::Default",
    },
    Feature {
        name: "rounded node A(label)",
        support: Support::Partial,
        note: "parsed and label extracted; shape maps to NodeKind::Default (no round corners rendered)",
    },
    Feature {
        name: "bare node A",
        support: Support::Supported,
        note: "auto-declared with id as label (Mermaid convention)",
    },
    Feature {
        name: "node label first-occurrence wins",
        support: Support::Supported,
        note: "subsequent A[other label] references do not overwrite the first label",
    },
    Feature {
        name: "stadium / circle node shape",
        support: Support::Unsupported,
        note: "`([label])` / `((label))` produce an explicit unsupported error",
    },
    // --- Flowchart edges ---
    Feature {
        name: "arrow edge -->",
        support: Support::Supported,
        note: "maps to ArrowType::Triangle",
    },
    Feature {
        name: "plain line edge ---",
        support: Support::Supported,
        note: "maps to ArrowType::None (no arrowhead drawn)",
    },
    Feature {
        name: "pipe edge label -->|label|",
        support: Support::Supported,
        note: "",
    },
    Feature {
        name: "space edge label -- label -->",
        support: Support::Supported,
        note: "",
    },
    Feature {
        name: "chain notation A --> B --> C",
        support: Support::Supported,
        note: "generates one edge per link in the chain",
    },
    Feature {
        name: "multi-target edge A --> B & C",
        support: Support::Unsupported,
        note: "reports an unsupported error; split into separate edge lines",
    },
    Feature {
        name: "self-loop A --> A",
        support: Support::Unsupported,
        note: "reports a clear error matching kozue DSL convention",
    },
    Feature {
        name: "dotted / thick edge styles",
        support: Support::Unsupported,
        note: "-.-> and ==> produce a syntax error",
    },
    // --- Sequence diagram ---
    Feature {
        name: "participant X as Label",
        support: Support::Supported,
        note: "",
    },
    Feature {
        name: "participant X (no label)",
        support: Support::Supported,
        note: "id used as label",
    },
    Feature {
        name: "auto-declare undeclared participants",
        support: Support::Supported,
        note: "first-message auto-declaration follows Mermaid convention",
    },
    Feature {
        name: "solid filled arrow ->>",
        support: Support::Supported,
        note: "LineStyle::Solid + ArrowType::Triangle",
    },
    Feature {
        name: "dashed filled arrow -->>",
        support: Support::Supported,
        note: "LineStyle::Dashed + ArrowType::Triangle",
    },
    Feature {
        name: "solid open arrow ->",
        support: Support::Partial,
        note: "open arrowhead not rendered; maps to Triangle (same as ->>)",
    },
    Feature {
        name: "dashed open arrow -->",
        support: Support::Partial,
        note: "open arrowhead not rendered; maps to Triangle (same as -->>)",
    },
    Feature {
        name: "self-message A->>A",
        support: Support::Supported,
        note: "",
    },
    Feature {
        name: "Note over / Note left / Note right",
        support: Support::Unsupported,
        note: "reports an unsupported error",
    },
    Feature {
        name: "loop block",
        support: Support::Unsupported,
        note: "reports an unsupported error",
    },
    Feature {
        name: "alt / else block",
        support: Support::Unsupported,
        note: "reports an unsupported error",
    },
    Feature {
        name: "activate / deactivate",
        support: Support::Unsupported,
        note: "reports an unsupported error",
    },
    // --- State diagram ---
    Feature {
        name: "state [*] --> S / S --> [*]",
        support: Support::Supported,
        note: "[*] as source maps to the initial pseudostate, as target to the final pseudostate",
    },
    Feature {
        name: "state transition label S --> T : label",
        support: Support::Supported,
        note: "text after the colon becomes the transition label",
    },
    Feature {
        name: "state \"description\" as id",
        support: Support::Supported,
        note: "quoted display name used as label; bare `state id` also accepted",
    },
    Feature {
        name: "auto-declare states in transitions",
        support: Support::Supported,
        note: "states referenced only in transitions are declared with id as label",
    },
    Feature {
        name: "[*] --> [*]",
        support: Support::Unsupported,
        note: "reports an error; initial cannot transition directly to final",
    },
    Feature {
        name: "composite / nested state s { … }",
        support: Support::Unsupported,
        note: "reports an unsupported error; no nested regions",
    },
    Feature {
        name: "fork / join / choice / history <<…>>",
        support: Support::Unsupported,
        note: "stereotype pseudostates report an unsupported error",
    },
    Feature {
        name: "state direction",
        support: Support::Unsupported,
        note: "reports an unsupported error; kozue lays state diagrams top-down",
    },
    Feature {
        name: "state note",
        support: Support::Unsupported,
        note: "reports an unsupported error",
    },
    Feature {
        name: "state description (S : text)",
        support: Support::Unsupported,
        note: "reports an unsupported error; internal state text not rendered",
    },
    // --- Common ---
    Feature {
        name: "%% comments",
        support: Support::Supported,
        note: "comment lines are stripped before parsing",
    },
    Feature {
        name: "blank lines and indentation",
        support: Support::Supported,
        note: "",
    },
    Feature {
        name: "semicolon separator",
        support: Support::Unsupported,
        note: "reports an unsupported error; use newlines",
    },
    Feature {
        name: "subgraph",
        support: Support::Unsupported,
        note: "reports an unsupported error",
    },
    Feature {
        name: "classDef / class / style / linkStyle",
        support: Support::Unsupported,
        note: "styling keywords report an unsupported error",
    },
];
