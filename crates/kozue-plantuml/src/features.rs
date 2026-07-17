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
        name: "class diagram",
        support: Support::Supported,
        note: "detected when the body uses `class`/`interface`/`abstract`/`enum` or a UML relation symbol (`<|`, `|>`, `*--`, `o--`); parsed into ClassDiagram IR",
    },
    Feature {
        name: "entity-relationship (ER) diagram",
        support: Support::Supported,
        note: "detected when the body has an `entity NAME {` block or a crow's-foot relation token; parsed into ErDiagram IR",
    },
    Feature {
        name: "diagram-kind inference",
        support: Support::Partial,
        note: "a body with none of the state/class/ER markers (only `A --> B` lines) is read as a sequence diagram, since that is ambiguous with a dashed message",
    },
    Feature {
        name: "ambiguous body (markers for 2+ kinds)",
        support: Support::Unsupported,
        note: "reports an explicit \"ambiguous @startuml body\" error rather than silently guessing",
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
        note: "ParticipantKind::Actor preserved in IR; stereotype rendered as «actor»",
    },
    Feature {
        name: "boundary / control / entity / database / collections / queue",
        support: Support::Supported,
        note: "icon-variant keywords map to the corresponding ParticipantKind; stereotype rendered as «kind»",
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
        note: "LineStyle::Solid + MessageArrow::Filled head",
    },
    Feature {
        name: "dashed arrow -->",
        support: Support::Supported,
        note: "LineStyle::Dashed + MessageArrow::Filled head",
    },
    Feature {
        name: "thin/async arrow ->> / -->>",
        support: Support::Supported,
        note: "open V arrowhead (MessageArrow::Open head)",
    },
    Feature {
        name: "bidirectional arrow <-> / <-->",
        support: Support::Supported,
        note: "filled head + filled tail",
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
        support: Support::Supported,
        note: "cross (lost) / circle (found) head markers; circle drawn as a small filled polygon until an ellipse primitive exists",
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
        name: "note over / left of / right of (single line)",
        support: Support::Supported,
        note: "single-line `note over/left of/right of ... : text`; multi-line note blocks, hnote, and rnote report an unsupported error",
    },
    Feature {
        name: "hnote / rnote / multi-line note block",
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
        support: Support::Supported,
        note: "`== text ==` parsed into SequenceItem::Divider",
    },
    Feature {
        name: "... / ...text... delays",
        support: Support::Supported,
        note: "`...` / `...text...` / `...text` parsed into SequenceItem::Delay; `||` spacers stay unsupported",
    },
    Feature {
        name: "ref over a[, b...] : text (single line)",
        support: Support::Supported,
        note: "single-line reference frame parsed into SequenceItem::Reference; multi-line `ref over a` blocks report an unsupported error",
    },
    Feature {
        name: "! preprocessor directives",
        support: Support::Unsupported,
        note: "any line starting with ! reports unsupported; kozue targets a preprocessor-free subset",
    },
    // --- Class diagram ---
    Feature {
        name: "class Foo / class Foo { ... }",
        support: Support::Supported,
        note: "multi-line or single-line `{ +a; +b }` (members split by newline or `;`); visibility markers (+ - # ~)",
    },
    Feature {
        name: "interface / abstract [class] / enum",
        support: Support::Supported,
        note: "maps to stereotype \"interface\" / \"abstract\" / \"enumeration\"",
    },
    Feature {
        name: "class relations, both spelling directions",
        support: Support::Supported,
        note: "<|-- --|> <|.. ..|> *-- --* o-- --o --> <-- ..> <.. -- .. all supported",
    },
    Feature {
        name: "class relation multiplicity A \"1\" -- \"*\" B",
        support: Support::Supported,
        note: "quoted multiplicity next to each class; also with markers/labels",
    },
    Feature {
        name: "generic type parameters ~T~",
        support: Support::Unsupported,
        note: "reports an unsupported error",
    },
    Feature {
        name: "namespace / note / hnote / rnote / package (class)",
        support: Support::Unsupported,
        note: "reports an unsupported error",
    },
    // --- ER diagram ---
    Feature {
        name: "entity Foo { [*] name : type }",
        support: Support::Supported,
        note: "multi-line or single-line `{ a; b }`; leading `*` marks a primary key; a bare `--` PK-separator line is skipped",
    },
    Feature {
        name: "crow's-foot relation A ||--o{ B : label",
        support: Support::Supported,
        note: "same glyph table as kozue-mermaid's erDiagram",
    },
    Feature {
        name: "plain (non-crow's-foot) relation A -- B",
        support: Support::Supported,
        note: "no end markers; `--` = solid, `..` = dashed — PlantUML's ER subset is smaller than Mermaid's",
    },
    Feature {
        name: "entity Foo (no `{ ... }` block)",
        support: Support::Unsupported,
        note: "an `entity` participant declaration without a block stays a sequence participant, not an ER entity",
    },
];
