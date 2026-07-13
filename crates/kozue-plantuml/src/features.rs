//! PlantUML compatibility feature table.
//!
//! This is the single source of truth for what kozue-plantuml supports. It is
//! used by `kozue compat plantuml` to display a table.

/// Support level of a PlantUML feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    /// Fully supported; output matches expected semantics.
    Supported,
    /// Partially supported; some aspects may differ from PlantUML.
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

/// A single PlantUML feature entry in the compatibility table.
#[derive(Debug, Clone)]
pub struct Feature {
    /// Short human-readable feature name.
    pub name: &'static str,
    /// Support level.
    pub support: Support,
    /// A note explaining the support level or any caveats.
    pub note: &'static str,
}

/// The full PlantUML compatibility table for the sequence diagram subset.
///
/// One canonical list — display, docs, and tests all derive from this.
pub const FEATURES: &[Feature] = &[
    // --- Diagram types ---
    Feature {
        name: "@startuml / @enduml",
        support: Support::Supported,
        note: "required delimiters; optional diagram name after @startuml is accepted and ignored",
    },
    Feature {
        name: "sequence diagram",
        support: Support::Supported,
        note: "M4 target: PlantUML sequence diagrams parsed into SequenceDiagram IR",
    },
    Feature {
        name: "state diagram",
        support: Support::Supported,
        note: "detected when the @startuml body uses `[*]` or a `state` declaration; parsed into StateDiagram IR",
    },
    Feature {
        name: "diagram-kind inference",
        support: Support::Partial,
        note: "a body with no `[*]` and no `state` keyword (only `A --> B` lines) is read as a sequence diagram, since that is ambiguous with a dashed message",
    },
    Feature {
        name: "@startmindmap / @startgantt / @startjson / etc.",
        support: Support::Unsupported,
        note: "non-sequence @start<type> delimiters produce a clear unsupported error",
    },
    // --- Participant declarations ---
    Feature {
        name: "participant Name",
        support: Support::Supported,
        note: "id and label both set to Name",
    },
    Feature {
        name: "participant Name as Alias",
        support: Support::Supported,
        note: "id set to Alias, label set to Name",
    },
    Feature {
        name: "participant \"Quoted Display\" as Alias",
        support: Support::Supported,
        note: "quoted display name used as label, Alias used as id",
    },
    Feature {
        name: "actor Name / actor \"...\" as X",
        support: Support::Supported,
        note: "same handling as participant; actor icon not rendered",
    },
    Feature {
        name: "boundary / control / entity / database / collections / queue",
        support: Support::Partial,
        note: "icon-variant keywords parsed and mapped to Participant; icon not rendered",
    },
    Feature {
        name: "auto-declare undeclared participants",
        support: Support::Supported,
        note: "participants used in messages but not declared are auto-declared on first use",
    },
    Feature {
        name: "duplicate participant id",
        support: Support::Unsupported,
        note: "declaring the same id twice reports an error (matches the kozue DSL frontend); no silent overwrite",
    },
    // --- Messages / arrows ---
    Feature {
        name: "solid arrow ->",
        support: Support::Supported,
        note: "LineStyle::Solid + ArrowType::Triangle",
    },
    Feature {
        name: "dashed arrow -->",
        support: Support::Supported,
        note: "LineStyle::Dashed + ArrowType::Triangle",
    },
    Feature {
        name: "solid filled arrow ->>",
        support: Support::Partial,
        note: "open/thin arrowhead not rendered; maps to Triangle (same as ->)",
    },
    Feature {
        name: "dashed filled arrow -->>",
        support: Support::Partial,
        note: "open/thin arrowhead not rendered; maps to Triangle (same as -->)",
    },
    Feature {
        name: "self-message A -> A : text",
        support: Support::Supported,
        note: "from and to participant are the same",
    },
    Feature {
        name: "message label A -> B : label",
        support: Support::Supported,
        note: "label text after colon is preserved",
    },
    Feature {
        name: "message without label A -> B",
        support: Support::Supported,
        note: "label is None",
    },
    Feature {
        name: "lost/found messages ->x / ->o",
        support: Support::Unsupported,
        note: "reports an unsupported error; do not misparse the arrow",
    },
    Feature {
        name: "colored arrows -[#red]>",
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
        name: "state transition S --> T : label / S -> T",
        support: Support::Supported,
        note: "both `-->` and `->` are transitions in a state diagram; text after the colon is the label",
    },
    Feature {
        name: "state S / state \"desc\" as S",
        support: Support::Supported,
        note: "bare and quoted-alias declarations; auto-declaration for states used only in transitions",
    },
    Feature {
        name: "[*] --> [*]",
        support: Support::Unsupported,
        note: "reports an error; initial cannot transition directly to final",
    },
    Feature {
        name: "composite state s { … }",
        support: Support::Unsupported,
        note: "reports an unsupported error; no nested regions or concurrency (`--`)",
    },
    Feature {
        name: "fork / join / choice / history <<…>>",
        support: Support::Unsupported,
        note: "stereotype pseudostates report an unsupported error",
    },
    Feature {
        name: "state description (S : text)",
        support: Support::Unsupported,
        note: "reports an unsupported error; internal state text not rendered",
    },
    // --- Comments ---
    Feature {
        name: "' line comments",
        support: Support::Supported,
        note: "single-quote starts a comment to end-of-line; comment masking is quote-aware, so ' / /' / '/ inside a \"...\" string are literal",
    },
    Feature {
        name: "/' ... '/ block comments",
        support: Support::Supported,
        note: "multi-line block comments are masked before parsing; a /' inside a string or line comment does not open a block",
    },
    // --- Unsupported keywords ---
    Feature {
        name: "note / hnote / rnote",
        support: Support::Unsupported,
        note: "reports an unsupported error",
    },
    Feature {
        name: "alt / else / opt",
        support: Support::Unsupported,
        note: "reports an unsupported error",
    },
    Feature {
        name: "loop",
        support: Support::Unsupported,
        note: "reports an unsupported error",
    },
    Feature {
        name: "par / break / critical / group",
        support: Support::Unsupported,
        note: "reports an unsupported error",
    },
    Feature {
        name: "end (block closer)",
        support: Support::Partial,
        note: "silently skipped (closes unsupported alt/loop/opt blocks); same as kozue-mermaid behaviour",
    },
    Feature {
        name: "activate / deactivate / destroy / create / return",
        support: Support::Unsupported,
        note: "reports an unsupported error",
    },
    Feature {
        name: "autonumber",
        support: Support::Unsupported,
        note: "reports an unsupported error",
    },
    Feature {
        name: "title / header / footer",
        support: Support::Unsupported,
        note: "reports an unsupported error",
    },
    Feature {
        name: "newpage / box / ref",
        support: Support::Unsupported,
        note: "reports an unsupported error",
    },
    Feature {
        name: "hide / show / skinparam",
        support: Support::Unsupported,
        note: "reports an unsupported error",
    },
    Feature {
        name: "== dividers",
        support: Support::Unsupported,
        note: "reports an unsupported error",
    },
    Feature {
        name: "... / || delays",
        support: Support::Unsupported,
        note: "reports an unsupported error",
    },
    Feature {
        name: "! preprocessor directives",
        support: Support::Unsupported,
        note: "any line starting with ! reports unsupported; kozue targets a preprocessor-free subset",
    },
];
