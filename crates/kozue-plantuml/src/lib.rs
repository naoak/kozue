//! PlantUML-syntax frontend for kozue.
//!
//! Parses a subset of PlantUML sequence diagrams into the
//! [`kozue_ir::Diagram`] semantic IR used by the native kozue DSL. Layout and
//! rendering are handled by the existing kozue pipeline unchanged.
//!
//! # Scope (M4)
//!
//! M4 targets **PlantUML sequence diagrams only** — preprocessor-free. Because
//! PlantUML does not declare the diagram type in a dedicated header line (the
//! same `A -> B` syntax is valid in sequence, component, and other diagrams),
//! auto-guessing the diagram kind would violate the never-silently-misparse
//! principle. Therefore component, state, class, activity, and all other
//! non-sequence diagram types are explicitly **out of scope** and produce clear
//! "unsupported" errors.
//!
//! # Supported syntax
//!
//! ```text
//! @startuml
//! ' single-line comment
//! /' block comment '/
//! participant Alice
//! participant Bob as B
//! participant "長い名前" as LN
//! actor User
//! boundary SomeService
//!
//! Alice -> B : こんにちは
//! B --> Alice : 返事
//! Alice ->> Alice : 自己メッセージ
//! @enduml
//! ```
//!
//! # Compatibility notes
//!
//! - `->`/`-->` map to a filled head; `->>`/`-->>` thin arrowheads map to an
//!   open head (`MessageArrow::Open`); `->x`/`->o` lost/found markers map to
//!   cross/circle heads; `<->`/`<-->` map to filled head + filled tail.
//! - Icon-variant keywords (`boundary`, `control`, `entity`, `database`,
//!   `collections`, `queue`) are parsed and mapped to `Participant`; the icon
//!   is not rendered.
//! - Unsupported features (notes, alt/loop/opt blocks, activate/deactivate,
//!   skinparam, preprocessor directives, non-sequence @start<type>, etc.) are
//!   reported as positioned "unsupported" errors rather than crashing or
//!   silently ignoring.
//! - `end` that closes an unsupported block is silently skipped (same as
//!   kozue-mermaid behaviour).

pub mod features;

use std::ops::Range;

use ariadne::{Label, Report, ReportKind, Source};
use kozue_ir::{
    ClassDiagram, ClassNode, ClassRelation, Diagram, Direction, EndMarker, Endpoint, ErAttribute,
    ErDiagram, ErEntity, ErRelation, IrDocument, LineStyle, Message, MessageArrow, Participant,
    ParticipantKind, SequenceDiagram, SequenceItem, State, StateDiagram, Transition,
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

/// Parse PlantUML source text into a semantic [`Diagram`].
///
/// Returns `Ok(diagram)` on success, or `Err(diagnostics)` where diagnostics
/// is a non-empty list of errors (all errors from the whole source are
/// collected before returning, following the same convention as `kozue-dsl`).
fn parse_diagram(source: &str) -> Result<Diagram, Vec<Diagnostic>> {
    let mut errors: Vec<Diagnostic> = Vec::new();

    // Strip UTF-8 BOM if present.
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);

    // Mask all comments (block `/' '/` and line `'`) with spaces, preserving byte
    // length and newlines so diagnostic spans map back to the original source.
    // Quote-aware: `'` / `/'` / `'/` inside a `"..."` string are literal.
    let (masked, unterminated_block) = mask_comments(source);

    // An unterminated block comment masks everything to EOF; surface it explicitly
    // rather than silently dropping the swallowed content.
    if let Some(span) = unterminated_block {
        errors.push(Diagnostic::new(
            "unterminated block comment: `/'` was never closed with `'/`",
            span,
        ));
    }

    // Tokenise into logical lines (drop blank lines), keeping original byte offsets.
    let lines: Vec<(usize, String)> = logical_lines(&masked);

    if lines.is_empty() {
        errors.push(Diagnostic::new(
            "empty diagram: expected `@startuml` header",
            0..source.len().max(1),
        ));
        return Err(errors);
    }

    // First non-comment logical line MUST be @startuml (optionally @startuml SomeName).
    let (header_offset, ref header_line) = lines[0];
    let header_trimmed = header_line.trim();

    // Check for @start<other> variants first.
    if let Some(rest) = header_trimmed.strip_prefix("@start") {
        let kind = rest.split_whitespace().next().unwrap_or(rest);
        if !kind.eq_ignore_ascii_case("uml") {
            errors.push(Diagnostic::new(
                format!(
                    "unsupported: @start{} diagram type; kozue-plantuml only supports @startuml (sequence diagrams)",
                    kind
                ),
                header_offset..header_offset + header_line.len(),
            ));
            return Err(errors);
        }
        // It is @startuml. Any trailing text is PlantUML's optional diagram *name*,
        // which is purely a document title with no bearing on the sequence's
        // semantics or layout — discarding it loses no diagram content, so no
        // diagnostic is warranted (documented in features.rs).
    } else {
        errors.push(Diagnostic::new(
            format!(
                "missing `@startuml`: first non-comment line must be `@startuml`, got `{}`",
                header_trimmed.chars().take(40).collect::<String>()
            ),
            header_offset..header_offset + header_line.len(),
        ));
        return Err(errors);
    }

    // Last non-comment logical line MUST be @enduml.
    let (enduml_offset, ref enduml_line) = lines[lines.len() - 1];
    let enduml_trimmed = enduml_line.trim();
    let has_enduml = enduml_trimmed.eq_ignore_ascii_case("@enduml");
    if !has_enduml {
        errors.push(Diagnostic::new(
            "missing `@enduml`: last non-comment line must be `@enduml`",
            enduml_offset..enduml_offset + enduml_line.len(),
        ));
        // Keep going to collect body errors, but use a sentinel empty body.
    }

    // Body is everything between @startuml and @enduml.
    let body_end = if has_enduml {
        lines.len() - 1
    } else {
        lines.len()
    };
    let body = &lines[1..body_end];

    // PlantUML uses `@startuml` for every diagram type; the kind is inferred from
    // the body syntax via a set of mutually exclusive markers. Zero matches
    // defaults to sequence (the historical M4 behaviour); exactly one match
    // selects that kind; two or more is a genuine ambiguity that must be
    // reported rather than silently guessed.
    let is_state = body_is_state(body);
    let is_class = body_is_class(body);
    let is_er = body_is_er(body);
    let matched: Vec<&str> = [
        (is_state, "state"),
        (is_class, "class"),
        (is_er, "entity/ER"),
    ]
    .into_iter()
    .filter_map(|(m, name)| m.then_some(name))
    .collect();

    if matched.len() >= 2 {
        errors.push(Diagnostic::new(
            format!(
                "ambiguous @startuml body: contains markers for both {} diagrams; kozue cannot infer the diagram kind",
                matched.join(" and ")
            ),
            header_offset..header_offset + header_line.len(),
        ));
        return Err(errors);
    }

    if is_state {
        parse_state_body(body, source, &mut errors);
        if errors.is_empty() {
            Ok(parse_state_clean(body))
        } else {
            Err(errors)
        }
    } else if is_class {
        parse_class_body(body, source, &mut errors);
        if errors.is_empty() {
            Ok(parse_class_clean(body))
        } else {
            Err(errors)
        }
    } else if is_er {
        parse_er_body(body, source, &mut errors);
        if errors.is_empty() {
            Ok(parse_er_clean(body))
        } else {
            Err(errors)
        }
    } else {
        parse_sequence_body(body, source, &mut errors);
        if errors.is_empty() {
            // Re-parse cleanly to produce the diagram (errors already empty).
            Ok(parse_sequence_clean(body))
        } else {
            Err(errors)
        }
    }
}

/// Parse PlantUML source text into a versioned semantic IR document.
pub fn parse_document(source: &str) -> Result<IrDocument, Vec<Diagnostic>> {
    let diagram = parse_diagram(source)?;
    let mut document = IrDocument::new(diagram);
    document.metadata.name = plantuml_name(source);
    Ok(document)
}

/// Parse PlantUML source text into a semantic [`Diagram`].
pub fn parse(source: &str) -> Result<Diagram, Vec<Diagnostic>> {
    parse_document(source).map(IrDocument::into_diagram)
}

fn plantuml_name(source: &str) -> Option<String> {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let (masked, _) = mask_comments(source);
    let lines = logical_lines(&masked);
    let header = lines.first()?.1.trim();
    let name = header.get("@startuml".len()..)?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// Does this body use state-diagram syntax? True when any logical line has a
/// `[*]` pseudostate endpoint or begins with the `state` keyword.
///
/// The `[*]` check only inspects the part before a `:` label: a `[*]` inside a
/// message/transition label (e.g. a sequence `A -> B : mark [*]`) is literal text,
/// not a diagram-kind signal, so it must not misroute a sequence diagram here.
fn body_is_state(lines: &[(usize, String)]) -> bool {
    lines.iter().any(|(_, line)| {
        let trimmed = line.trim();
        let head = trimmed.split(':').next().unwrap_or(trimmed);
        head.contains("[*]")
            || split_keyword(trimmed)
                .map(|(kw, _)| kw.eq_ignore_ascii_case("state"))
                .unwrap_or(false)
    })
}

/// Does this body use class-diagram syntax? True when a line begins with the
/// `class`/`interface`/`abstract`/`enum` keyword, or contains a UML-specific
/// relation symbol (`<|`, `|>`, `*--`, `--*`, `o--`, `--o`). A bare `-->` is
/// NOT treated as a class-diagram signal (it is equally valid sequence/state
/// syntax), so it never triggers this detector on its own.
fn body_is_class(lines: &[(usize, String)]) -> bool {
    lines.iter().any(|(_, line)| {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return false;
        }
        if let Some((kw, _)) = split_keyword(trimmed) {
            if matches!(
                kw.to_ascii_lowercase().as_str(),
                "class" | "interface" | "abstract" | "enum"
            ) {
                return true;
            }
        }
        // Check token-by-token (not the whole line) so a crow's-foot ER token
        // like `||--o{` — which contains the substring `--o` — is not
        // mistaken for the class-diagram aggregation marker `--o`.
        trimmed.split_whitespace().any(|tok| {
            parse_crowfoot_token(tok).is_none()
                && (tok.contains("<|")
                    || tok.contains("|>")
                    || tok.contains("*--")
                    || tok.contains("--*")
                    || tok.contains("o--")
                    || tok.contains("--o"))
        })
    })
}

/// Does this body use ER-diagram syntax? True when a line opens an `entity`
/// block — either multi-line `entity NAME {` or single-line
/// `entity NAME { ... }` (an `entity` participant declaration with *no* `{` at
/// all stays a sequence participant — this is what disambiguates the two) — or
/// contains a crow's-foot relation token with an actual crow's-foot glyph.
///
/// Detection deliberately ignores the bare `--`/`..` connectors: although
/// [`parse_crowfoot_token`] accepts them **inside** an already-identified ER
/// body, they are far too generic to signal the diagram *kind* — a plain
/// `A -- B` is equally valid class/sequence syntax, so treating it as an ER
/// signal would spuriously flag class bodies (e.g. `class A ... A -- B`) as
/// ambiguous.
fn body_is_er(lines: &[(usize, String)]) -> bool {
    lines.iter().any(|(_, line)| {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return false;
        }
        if let Some((kw, rest)) = split_keyword(trimmed) {
            if kw.eq_ignore_ascii_case("entity") && rest.contains('{') {
                return true;
            }
        }
        trimmed
            .split_whitespace()
            .any(|tok| tok != "--" && tok != ".." && parse_crowfoot_token(tok).is_some())
    })
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
// Sequence body parser (error-collecting pass)
// ---------------------------------------------------------------------------

fn parse_sequence_body(lines: &[(usize, String)], _source: &str, errors: &mut Vec<Diagnostic>) {
    // Ids introduced by explicit `participant`/`actor`/… declarations, to detect
    // duplicate declarations (matching the kozue-dsl frontend, which rejects them).
    let mut declared_ids: Vec<String> = Vec::new();
    for (offset, line) in lines {
        let trimmed = line.trim();
        let line_end = offset + line.len();
        let span = *offset..line_end;

        if trimmed.is_empty() {
            continue;
        }

        // Preprocessor lines: any line whose first non-space char is `!`.
        if trimmed.starts_with('!') {
            errors.push(Diagnostic::new(
                "unsupported: PlantUML preprocessor directives (`!...`) are not supported; kozue targets a preprocessor-free subset",
                span,
            ));
            continue;
        }

        // `== divider ==` lines.
        if trimmed.starts_with("==") {
            errors.push(Diagnostic::new(
                "unsupported: == dividers are not supported",
                span,
            ));
            continue;
        }

        // `...` or `||` delay lines.
        if trimmed == "..." || trimmed.starts_with("...") || trimmed.starts_with("||") {
            errors.push(Diagnostic::new(
                "unsupported: delay markers (`...` / `||`) are not supported",
                span,
            ));
            continue;
        }

        // Participant / actor / icon-variant keyword declarations.
        if let Some((id, _, _)) = try_parse_participant_decl(trimmed) {
            if declared_ids.iter().any(|d| d == &id) {
                errors.push(Diagnostic::new(
                    format!("duplicate participant id `{id}`"),
                    span.clone(),
                ));
            } else {
                declared_ids.push(id);
            }
            continue;
        }

        // Message arrow lines.
        if let Some(result) = try_parse_plantuml_message(trimmed) {
            match result {
                Ok(_) => {
                    // Valid — no error.
                }
                Err(msg) => {
                    errors.push(Diagnostic::new(msg, span));
                }
            }
            continue;
        }

        // `end` silently skipped (closes unsupported alt/loop/opt/group blocks).
        if trimmed == "end" {
            continue;
        }

        // Unsupported keywords — word-boundary matched so `participant` isn't caught by `par`.
        let unsupported_kw: &[(&str, &str)] = &[
            (
                "note",
                "unsupported: note (kozue does not support this yet)",
            ),
            (
                "hnote",
                "unsupported: hnote (kozue does not support this yet)",
            ),
            (
                "rnote",
                "unsupported: rnote (kozue does not support this yet)",
            ),
            ("alt", "unsupported: alt (kozue does not support this yet)"),
            (
                "else",
                "unsupported: else (kozue does not support this yet)",
            ),
            ("opt", "unsupported: opt (kozue does not support this yet)"),
            (
                "loop",
                "unsupported: loop (kozue does not support this yet)",
            ),
            ("par", "unsupported: par (kozue does not support this yet)"),
            (
                "break",
                "unsupported: break (kozue does not support this yet)",
            ),
            (
                "critical",
                "unsupported: critical (kozue does not support this yet)",
            ),
            (
                "group",
                "unsupported: group (kozue does not support this yet)",
            ),
            (
                "activate",
                "unsupported: activate (kozue does not support this yet)",
            ),
            (
                "deactivate",
                "unsupported: deactivate (kozue does not support this yet)",
            ),
            (
                "destroy",
                "unsupported: destroy (kozue does not support this yet)",
            ),
            (
                "create",
                "unsupported: create (kozue does not support this yet)",
            ),
            (
                "return",
                "unsupported: return (kozue does not support this yet)",
            ),
            (
                "autonumber",
                "unsupported: autonumber (kozue does not support this yet)",
            ),
            (
                "title",
                "unsupported: title (kozue does not support this yet)",
            ),
            (
                "header",
                "unsupported: header (kozue does not support this yet)",
            ),
            (
                "footer",
                "unsupported: footer (kozue does not support this yet)",
            ),
            (
                "newpage",
                "unsupported: newpage (kozue does not support this yet)",
            ),
            ("box", "unsupported: box (kozue does not support this yet)"),
            ("ref", "unsupported: ref (kozue does not support this yet)"),
            (
                "hide",
                "unsupported: hide (kozue does not support this yet)",
            ),
            (
                "show",
                "unsupported: show (kozue does not support this yet)",
            ),
            (
                "skinparam",
                "unsupported: skinparam (kozue does not support this yet)",
            ),
        ];

        let mut found_unsupported = false;
        for &(kw, msg) in unsupported_kw {
            // Word-boundary: keyword must be followed by whitespace or end-of-string.
            if trimmed == kw
                || (trimmed.starts_with(kw)
                    && trimmed[kw.len()..].starts_with(|c: char| c.is_ascii_whitespace()))
            {
                errors.push(Diagnostic::new(msg, span.clone()));
                found_unsupported = true;
                break;
            }
        }
        if found_unsupported {
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
}

/// Clean parse pass — only called when parse_sequence_body found no errors.
fn parse_sequence_clean(lines: &[(usize, String)]) -> Diagram {
    let mut seq = SequenceDiagram::new();

    let ensure_participant =
        |seq: &mut SequenceDiagram, id: &str, label: Option<&str>, kind: ParticipantKind| {
            if !seq.participants.contains_key(id) {
                let lbl = label.unwrap_or(id).to_string();
                seq.participants
                    .insert(id.into(), Participant::with_kind(id, lbl, kind));
            }
        };

    for (_offset, line) in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed == "end" {
            continue;
        }

        if let Some((id, label, kind)) = try_parse_participant_decl(trimmed) {
            ensure_participant(&mut seq, &id, label.as_deref(), kind);
            continue;
        }

        if let Some(Ok((from, to, label, line_style, head, tail))) =
            try_parse_plantuml_message(trimmed)
        {
            ensure_participant(&mut seq, &from, None, ParticipantKind::Default);
            ensure_participant(&mut seq, &to, None, ParticipantKind::Default);
            seq.items.push(SequenceItem::Message(Message::with_arrows(
                from, to, label, line_style, head, tail,
            )));
        }
    }

    Diagram::Sequence(seq)
}

// ---------------------------------------------------------------------------
// State diagram parser
// ---------------------------------------------------------------------------

/// Keywords that are unsupported in a state-diagram body. Word-boundary matched.
const STATE_UNSUPPORTED_KW: &[&str] = &[
    "note",
    "hnote",
    "rnote",
    "hide",
    "show",
    "skinparam",
    "title",
    "header",
    "footer",
    "scale",
    "caption",
    "legend",
];

/// Error-collecting pass over a PlantUML state-diagram body. Mirrors
/// [`parse_sequence_body`]: it validates every line and pushes diagnostics, but
/// does not build the diagram (that is [`parse_state_clean`], run only when this
/// pass finds no errors).
fn parse_state_body(lines: &[(usize, String)], _source: &str, errors: &mut Vec<Diagnostic>) {
    let mut declared_ids: Vec<String> = Vec::new();
    for (offset, line) in lines {
        let trimmed = line.trim();
        let span = *offset..offset + line.len();

        if trimmed.is_empty() {
            continue;
        }

        // Preprocessor directives.
        if trimmed.starts_with('!') {
            errors.push(Diagnostic::new(
                "unsupported: PlantUML preprocessor directives (`!...`) are not supported; kozue targets a preprocessor-free subset",
                span,
            ));
            continue;
        }

        // Composite / nested states.
        if trimmed.contains('{') || trimmed == "}" {
            errors.push(Diagnostic::new(
                "unsupported: composite/nested state (`state s { … }`); kozue does not support this yet",
                span,
            ));
            continue;
        }

        // Fork / join / choice / history pseudostates.
        if trimmed.contains("<<") {
            errors.push(Diagnostic::new(
                "unsupported: fork/join/choice/history pseudostate (`<<…>>`); kozue does not support this yet",
                span,
            ));
            continue;
        }

        // `left to right direction` / `top to bottom direction`.
        if trimmed.eq_ignore_ascii_case("left to right direction")
            || trimmed.eq_ignore_ascii_case("top to bottom direction")
        {
            errors.push(Diagnostic::new(
                "unsupported: direction in state diagrams; kozue lays state diagrams top-down (kozue does not support this yet)",
                span,
            ));
            continue;
        }

        // Concurrency separator inside composite states.
        if trimmed == "--" || trimmed == "||" {
            errors.push(Diagnostic::new(
                "unsupported: concurrent region separator (`--` / `||`); kozue does not support this yet",
                span,
            ));
            continue;
        }

        // State declarations: `state id` or `state "desc" as id`.
        if let Some((kw, rest)) = split_keyword(trimmed) {
            if kw.eq_ignore_ascii_case("state") {
                match parse_state_decl_puml(rest.trim()) {
                    Ok((id, _label)) => {
                        if declared_ids.iter().any(|d| d == &id) {
                            errors.push(Diagnostic::new(
                                format!("duplicate state declaration `{id}`"),
                                span,
                            ));
                        } else {
                            declared_ids.push(id);
                        }
                    }
                    Err(msg) => errors.push(Diagnostic::new(msg, span)),
                }
                continue;
            }
        }

        // Transitions (contain a `->` / `-->` arrow).
        if let Some(result) = try_parse_state_transition(trimmed) {
            match result {
                Ok((from, to, _label)) => {
                    if matches!(from, Endpoint::Initial) && matches!(to, Endpoint::Final) {
                        errors.push(Diagnostic::new(
                            "`[*] --> [*]` is not valid; initial pseudostate cannot transition directly to final pseudostate",
                            span,
                        ));
                    }
                }
                Err(msg) => errors.push(Diagnostic::new(msg, span)),
            }
            continue;
        }

        // Unsupported keywords (note/hide/skinparam/…).
        let mut found_unsupported = false;
        for &kw in STATE_UNSUPPORTED_KW {
            if trimmed == kw
                || (trimmed.starts_with(kw)
                    && trimmed[kw.len()..].starts_with(|c: char| c.is_ascii_whitespace()))
            {
                errors.push(Diagnostic::new(
                    format!("unsupported: {kw} (kozue does not support this yet)"),
                    span.clone(),
                ));
                found_unsupported = true;
                break;
            }
        }
        if found_unsupported {
            continue;
        }

        // Unrecognised line (e.g. `S : description` state-body text).
        errors.push(Diagnostic::new(
            format!(
                "syntax error: unrecognised statement `{}`",
                trimmed.chars().take(40).collect::<String>()
            ),
            span,
        ));
    }
}

/// Clean pass — only called when [`parse_state_body`] found no errors.
fn parse_state_clean(lines: &[(usize, String)]) -> Diagram {
    let mut diagram = StateDiagram::new();

    let ensure_state = |diagram: &mut StateDiagram, id: &str, label: Option<&str>| {
        if !diagram.states.contains_key(id) {
            let lbl = label.unwrap_or(id).to_string();
            diagram.states.insert(id.into(), State::new(id, lbl));
        }
    };

    // Explicit declarations first (source order), so their labels win.
    for (_offset, line) in lines {
        let trimmed = line.trim();
        if let Some((kw, rest)) = split_keyword(trimmed) {
            if kw.eq_ignore_ascii_case("state") {
                if let Ok((id, label)) = parse_state_decl_puml(rest.trim()) {
                    ensure_state(&mut diagram, &id, Some(&label));
                }
                continue;
            }
        }
    }

    // Then transitions, auto-declaring any endpoints not seen above.
    for (_offset, line) in lines {
        let trimmed = line.trim();
        if let Some(Ok((from, to, label))) = try_parse_state_transition(trimmed) {
            for ep in [&from, &to] {
                if let Endpoint::State(id) = ep {
                    ensure_state(&mut diagram, id.as_str(), None);
                }
            }
            diagram.transitions.push(Transition::new(from, to, label));
        }
    }

    Diagram::State(diagram)
}

/// Parse the text after the `state` keyword: `"desc" as id` or a bare `id`.
fn parse_state_decl_puml(rest: &str) -> Result<(String, String), String> {
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
        let id = strip_keyword_boundary_ci(after, "as")
            .ok_or("expected `as <id>` after quoted state description")?
            .trim();
        if !is_single_token(id) {
            return Err(format!(
                "invalid state identifier `{}`",
                id.chars().take(40).collect::<String>()
            ));
        }
        Ok((id.to_string(), label))
    } else {
        // Bare `state id`. Reject `state s : desc` (state-description assignment)
        // and any trailing tokens.
        if rest.contains(':') {
            return Err(
                "unsupported: state description (`state s : …`); kozue does not support this yet"
                    .to_string(),
            );
        }
        if !is_single_token(rest) || !is_valid_participant_id(rest) {
            return Err(format!(
                "invalid state declaration `{}`",
                rest.chars().take(40).collect::<String>()
            ));
        }
        Ok((rest.to_string(), rest.to_string()))
    }
}

/// (from endpoint, to endpoint, optional transition label)
type StateTransResult = (Endpoint, Endpoint, Option<String>);

/// Try to parse a PlantUML state transition `FROM --> TO` / `FROM -> TO`,
/// optionally with a ` : label`. Returns `None` if the line has no arrow.
fn try_parse_state_transition(line: &str) -> Option<Result<StateTransResult, String>> {
    // Split off an optional `: label` FIRST, so an arrow sequence inside the
    // label text (e.g. `A -> B : x --> y`) is not mistaken for the transition
    // arrow. State identifiers never contain `:`, so the first colon is always
    // the label separator.
    let (endpoints, label) = match line.find(':') {
        Some(ci) => {
            let lbl = line[ci + 1..].trim();
            let label = if lbl.is_empty() {
                None
            } else {
                Some(lbl.to_string())
            };
            (line[..ci].trim(), label)
        }
        None => (line.trim(), None),
    };

    // Longest arrow first so `-->` is not split as `->`.
    let arrow = if endpoints.contains("-->") {
        "-->"
    } else if endpoints.contains("->") {
        "->"
    } else {
        return None;
    };

    let idx = endpoints.find(arrow).expect("arrow presence just checked");
    let from_part = endpoints[..idx].trim();
    let to_part = endpoints[idx + arrow.len()..].trim();

    let from = match parse_state_endpoint_puml(from_part, true) {
        Ok(ep) => ep,
        Err(e) => return Some(Err(e)),
    };
    let to = match parse_state_endpoint_puml(to_part, false) {
        Ok(ep) => ep,
        Err(e) => return Some(Err(e)),
    };
    Some(Ok((from, to, label)))
}

/// Parse a transition endpoint. `[*]` maps to [`Endpoint::Initial`] as a source
/// and [`Endpoint::Final`] as a target; otherwise a bare state identifier.
fn parse_state_endpoint_puml(part: &str, is_source: bool) -> Result<Endpoint, String> {
    let part = part.trim();
    if part == "[*]" {
        return Ok(if is_source {
            Endpoint::Initial
        } else {
            Endpoint::Final
        });
    }
    if is_single_token(part) && is_valid_participant_id(part) {
        Ok(Endpoint::State(part.into()))
    } else {
        Err(format!(
            "syntax error: expected a state identifier or `[*]`, got `{}`",
            part.chars().take(40).collect::<String>()
        ))
    }
}

// ---------------------------------------------------------------------------
// Class diagram parser
// ---------------------------------------------------------------------------

/// PlantUML class-diagram relation connectors, both spelling directions:
/// `(token, from_marker, to_marker, dashed)`.
const CLASS_CONNECTORS: &[(&str, EndMarker, EndMarker, bool)] = &[
    ("<|--", EndMarker::HollowTriangle, EndMarker::None, false),
    ("<|..", EndMarker::HollowTriangle, EndMarker::None, true),
    ("--|>", EndMarker::None, EndMarker::HollowTriangle, false),
    ("..|>", EndMarker::None, EndMarker::HollowTriangle, true),
    ("*--", EndMarker::FilledDiamond, EndMarker::None, false),
    ("--*", EndMarker::None, EndMarker::FilledDiamond, false),
    ("o--", EndMarker::HollowDiamond, EndMarker::None, false),
    ("--o", EndMarker::None, EndMarker::HollowDiamond, false),
    ("-->", EndMarker::None, EndMarker::OpenArrow, false),
    ("<--", EndMarker::OpenArrow, EndMarker::None, false),
    ("..>", EndMarker::None, EndMarker::OpenArrow, true),
    ("<..", EndMarker::OpenArrow, EndMarker::None, true),
    ("--", EndMarker::None, EndMarker::None, false),
    ("..", EndMarker::None, EndMarker::None, true),
];

/// Try to decode the text after a class/interface/abstract/enum keyword into
/// (optional stereotype, name-and-rest). `abstract class Foo` and bare
/// `abstract Foo` both map to the `"abstract"` stereotype.
fn plantuml_class_decl(trimmed: &str) -> Option<(Option<&'static str>, String)> {
    let (kw, rest) = split_keyword(trimmed)?;
    match kw.to_ascii_lowercase().as_str() {
        "class" => Some((None, rest.to_string())),
        "interface" => Some((Some("interface"), rest.to_string())),
        "enum" => Some((Some("enumeration"), rest.to_string())),
        "abstract" => {
            if let Some((kw2, rest2)) = split_keyword(rest) {
                if kw2.eq_ignore_ascii_case("class") {
                    return Some((Some("abstract"), rest2.to_string()));
                }
            }
            Some((Some("abstract"), rest.to_string()))
        }
        _ => None,
    }
}

/// The body shape of a `class`/`entity` declaration, derived from the text
/// after the name.
enum ClassBlock<'a> {
    /// `Foo` — no body.
    None,
    /// `Foo { +a; +b }` — single line, `;`-separated members.
    Inline(&'a str),
    /// `Foo {` — a multi-line block, closed by a standalone `}` line.
    Multiline,
}

/// Split the text after a `class`/`entity` keyword into its name and body
/// shape. Accepts `Foo`, `Foo {` (multi-line), and `Foo { ... }` (inline).
fn class_decl_body(rest: &str) -> (&str, ClassBlock<'_>) {
    if let Some(open) = rest.find('{') {
        if let Some(inner) = rest[open + 1..].strip_suffix('}') {
            (rest[..open].trim(), ClassBlock::Inline(inner.trim()))
        } else {
            (rest[..open].trim(), ClassBlock::Multiline)
        }
    } else {
        (rest, ClassBlock::None)
    }
}

/// Parse a class member line (visibility marker + attribute/method) into its
/// pre-formatted display string, matching kozue-mermaid's class member format.
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

struct ParsedClassRelation {
    from: String,
    to: String,
    from_marker: EndMarker,
    to_marker: EndMarker,
    dashed: bool,
    label: Option<String>,
    from_mult: Option<String>,
    to_mult: Option<String>,
}

/// Split a string into whitespace-separated tokens, treating a `"..."` run as
/// a single token so quoted multiplicities that contain spaces stay intact.
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

/// Split one side of a relation (the tokens before/after the connector) into
/// its identifier and optional multiplicity (a `"..."` token). The order is
/// free: PlantUML writes the multiplicity next to the class it annotates
/// (`A "1"` on the left, `"*" B` on the right).
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

/// Try to parse a class relation line: `A <|-- B`, `A --|> B : label`, or with
/// multiplicities `A "1" -- "*" B`. Returns `None` if the line has no
/// recognised connector token.
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
    let &(_, from_marker, to_marker, dashed) = CLASS_CONNECTORS
        .iter()
        .find(|(tok, ..)| *tok == tokens[conn_idx])
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
        dashed,
        label,
        from_mult,
        to_mult,
    }))
}

/// Error-collecting pass over a PlantUML class-diagram body.
fn parse_class_body(lines: &[(usize, String)], _source: &str, errors: &mut Vec<Diagnostic>) {
    let mut declared_ids: Vec<String> = Vec::new();
    let mut i = 0usize;
    'outer: while i < lines.len() {
        let (offset, line) = &lines[i];
        let trimmed = line.trim();
        let span = *offset..offset + line.len();

        if trimmed.is_empty() {
            i += 1;
            continue;
        }

        if trimmed.starts_with('!') {
            errors.push(Diagnostic::new(
                "unsupported: PlantUML preprocessor directives (`!...`) are not supported; kozue targets a preprocessor-free subset",
                span,
            ));
            i += 1;
            continue;
        }

        if trimmed.contains('~') {
            errors.push(Diagnostic::new(
                "unsupported: generic type parameters (`~T~`); kozue does not support this yet",
                span,
            ));
            i += 1;
            continue;
        }

        for kw in ["namespace", "note", "hnote", "rnote", "package"] {
            if split_keyword(trimmed)
                .map(|(k, _)| k.eq_ignore_ascii_case(kw))
                .unwrap_or(false)
            {
                errors.push(Diagnostic::new(
                    format!("unsupported: {kw} (kozue does not support this yet)"),
                    span.clone(),
                ));
                i += 1;
                if trimmed.ends_with('{') {
                    while i < lines.len() && lines[i].1.trim() != "}" {
                        i += 1;
                    }
                    i += 1;
                }
                continue 'outer;
            }
        }

        if let Some((_, rest)) = plantuml_class_decl(trimmed) {
            let (name, block) = class_decl_body(rest.trim());
            if !is_single_token(name) {
                errors.push(Diagnostic::new(
                    format!(
                        "syntax error: expected a class identifier, got `{}`",
                        name.chars().take(40).collect::<String>()
                    ),
                    span.clone(),
                ));
                i += 1;
                continue;
            }
            if declared_ids.iter().any(|d| d == name) {
                errors.push(Diagnostic::new(
                    format!("duplicate class declaration `{name}`"),
                    span.clone(),
                ));
            } else {
                declared_ids.push(name.to_string());
            }
            i += 1;
            if matches!(block, ClassBlock::Multiline) {
                loop {
                    if i >= lines.len() {
                        errors.push(Diagnostic::new(
                            format!("unterminated `{name} {{ ... }}` block (missing `}}`)"),
                            span.clone(),
                        ));
                        break;
                    }
                    let (moff, mline) = &lines[i];
                    let mtrim = mline.trim();
                    if mtrim == "}" {
                        i += 1;
                        break;
                    }
                    if mtrim.contains('~') {
                        errors.push(Diagnostic::new(
                            "unsupported: generic type parameters (`~T~`); kozue does not support this yet",
                            *moff..moff + mline.len(),
                        ));
                    }
                    i += 1;
                }
            }
            continue;
        }

        if let Some(result) = try_parse_class_relation(trimmed) {
            if let Err(msg) = result {
                errors.push(Diagnostic::new(msg, span));
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
}

/// Clean pass — only called when [`parse_class_body`] found no errors.
fn parse_class_clean(lines: &[(usize, String)]) -> Diagram {
    let mut diagram = ClassDiagram::new(Direction::Down);
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
        let (_offset, line) = &lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() {
            i += 1;
            continue;
        }

        if let Some((stereotype, rest)) = plantuml_class_decl(trimmed) {
            let (name, block) = class_decl_body(rest.trim());
            ensure_class(&mut diagram, name);
            if let Some(st) = stereotype {
                diagram.classes[name].stereotype = Some(st.to_string());
            }
            match block {
                ClassBlock::None => {
                    i += 1;
                }
                ClassBlock::Inline(inner) => {
                    for member in inner.split(';') {
                        let member = member.trim();
                        if member.is_empty() {
                            continue;
                        }
                        let node = &mut diagram.classes[name];
                        let mut attrs = std::mem::take(&mut node.attributes);
                        let mut methods = std::mem::take(&mut node.methods);
                        parse_class_member(member, &mut attrs, &mut methods);
                        let node = &mut diagram.classes[name];
                        node.attributes = attrs;
                        node.methods = methods;
                    }
                    i += 1;
                }
                ClassBlock::Multiline => {
                    i += 1;
                    loop {
                        if i >= lines.len() {
                            break;
                        }
                        let (_moff, mline) = &lines[i];
                        let mtrim = mline.trim();
                        if mtrim == "}" {
                            i += 1;
                            break;
                        }
                        let node = &mut diagram.classes[name];
                        let mut attrs = std::mem::take(&mut node.attributes);
                        let mut methods = std::mem::take(&mut node.methods);
                        parse_class_member(mtrim, &mut attrs, &mut methods);
                        let node = &mut diagram.classes[name];
                        node.attributes = attrs;
                        node.methods = methods;
                        i += 1;
                    }
                }
            }
            continue;
        }

        if let Some(Ok(rel)) = try_parse_class_relation(trimmed) {
            ensure_class(&mut diagram, &rel.from);
            ensure_class(&mut diagram, &rel.to);
            diagram.relations.push(ClassRelation::new(
                rel.from,
                rel.to,
                rel.from_marker,
                rel.to_marker,
                if rel.dashed {
                    LineStyle::Dashed
                } else {
                    LineStyle::Solid
                },
                rel.label,
                rel.from_mult,
                rel.to_mult,
            ));
        }
        i += 1;
    }

    Diagram::Class(diagram)
}

// ---------------------------------------------------------------------------
// ER diagram parser
// ---------------------------------------------------------------------------

/// Decode an ER relation connector token. Unlike Mermaid, a bare `--`/`..`
/// with no crow's-foot glyphs is accepted (PlantUML's ER subset is smaller —
/// EndMarker is only set `if crow's-foot glyphs are present`, per spec).
fn parse_crowfoot_token(tok: &str) -> Option<(EndMarker, EndMarker, LineStyle)> {
    if tok == "--" {
        return Some((EndMarker::None, EndMarker::None, LineStyle::Solid));
    }
    if tok == ".." {
        return Some((EndMarker::None, EndMarker::None, LineStyle::Dashed));
    }
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

/// Parse a PlantUML ER entity attribute line: `[*] name : type`.
/// A leading `*` marks the attribute as a primary key; other leading
/// visibility markers (`+`/`-`/`#`) are accepted and simply stripped.
fn parse_er_attr_line(trimmed: &str) -> Result<ErAttribute, String> {
    let (is_pk, rest) = match trimmed.strip_prefix('*') {
        Some(r) => (true, r.trim_start()),
        None => (
            false,
            trimmed
                .strip_prefix(['+', '-', '#'])
                .map(|r| r.trim_start())
                .unwrap_or(trimmed),
        ),
    };
    let Some(ci) = rest.find(':') else {
        return Err(format!(
            "syntax error: expected `name : type` in entity attribute, got `{}`",
            trimmed.chars().take(40).collect::<String>()
        ));
    };
    let name = rest[..ci].trim();
    let type_name = rest[ci + 1..].trim();
    if name.is_empty() {
        return Err("expected an attribute name before `:`".to_string());
    }
    let keys = if is_pk {
        vec!["PK".to_string()]
    } else {
        vec![]
    };
    Ok(ErAttribute::new(type_name, name, keys, None))
}

/// Error-collecting pass over a PlantUML ER-diagram body.
fn parse_er_body(lines: &[(usize, String)], _source: &str, errors: &mut Vec<Diagnostic>) {
    let mut i = 0usize;
    while i < lines.len() {
        let (offset, line) = &lines[i];
        let trimmed = line.trim();
        let span = *offset..offset + line.len();

        if trimmed.is_empty() {
            i += 1;
            continue;
        }

        if let Some((kw, rest)) = split_keyword(trimmed) {
            if kw.eq_ignore_ascii_case("entity") {
                let (name, block) = class_decl_body(rest.trim());
                if !is_single_token(name) {
                    errors.push(Diagnostic::new(
                        format!(
                            "syntax error: expected an entity identifier, got `{}`",
                            name.chars().take(40).collect::<String>()
                        ),
                        span,
                    ));
                    i += 1;
                    continue;
                }
                match block {
                    ClassBlock::None => {
                        errors.push(Diagnostic::new(
                            "unsupported: `entity` declaration without a `{ ... }` block in an ER-diagram body",
                            span,
                        ));
                        i += 1;
                        continue;
                    }
                    ClassBlock::Inline(inner) => {
                        for attr in inner.split(';') {
                            let attr = attr.trim();
                            if attr.is_empty() || attr == "--" {
                                continue;
                            }
                            if let Err(msg) = parse_er_attr_line(attr) {
                                errors.push(Diagnostic::new(msg, span.clone()));
                            }
                        }
                        i += 1;
                        continue;
                    }
                    ClassBlock::Multiline => {
                        i += 1;
                        loop {
                            if i >= lines.len() {
                                errors.push(Diagnostic::new(
                                    format!(
                                        "unterminated `{name} {{ ... }}` entity block (missing `}}`)"
                                    ),
                                    span.clone(),
                                ));
                                break;
                            }
                            let (moff, mline) = &lines[i];
                            let mtrim = mline.trim();
                            if mtrim == "}" {
                                i += 1;
                                break;
                            }
                            if mtrim == "--" {
                                // PlantUML separates the PK section with a bare `--`.
                                i += 1;
                                continue;
                            }
                            if let Err(msg) = parse_er_attr_line(mtrim) {
                                errors.push(Diagnostic::new(msg, *moff..moff + mline.len()));
                            }
                            i += 1;
                        }
                        continue;
                    }
                }
            }
        }

        if let Some(result) = try_parse_er_relation(trimmed) {
            if let Err(msg) = result {
                errors.push(Diagnostic::new(msg, span));
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
}

/// Clean pass — only called when [`parse_er_body`] found no errors.
fn parse_er_clean(lines: &[(usize, String)]) -> Diagram {
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
        let (_offset, line) = &lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() {
            i += 1;
            continue;
        }

        if let Some((kw, rest)) = split_keyword(trimmed) {
            if kw.eq_ignore_ascii_case("entity") {
                let (name, block) = class_decl_body(rest.trim());
                let name = name.to_string();
                match block {
                    ClassBlock::None => {}
                    ClassBlock::Inline(inner) => {
                        ensure_entity(&mut diagram, &name);
                        for attr in inner.split(';') {
                            let attr = attr.trim();
                            if attr.is_empty() || attr == "--" {
                                continue;
                            }
                            if let Ok(a) = parse_er_attr_line(attr) {
                                diagram.entities[name.as_str()].attributes.push(a);
                            }
                        }
                        i += 1;
                        continue;
                    }
                    ClassBlock::Multiline => {
                        ensure_entity(&mut diagram, &name);
                        i += 1;
                        loop {
                            if i >= lines.len() {
                                break;
                            }
                            let (_moff, mline) = &lines[i];
                            let mtrim = mline.trim();
                            if mtrim == "}" {
                                i += 1;
                                break;
                            }
                            if mtrim != "--" {
                                if let Ok(attr) = parse_er_attr_line(mtrim) {
                                    diagram.entities[name.as_str()].attributes.push(attr);
                                }
                            }
                            i += 1;
                        }
                        continue;
                    }
                }
            }
        }

        if let Some(Ok(rel)) = try_parse_er_relation(trimmed) {
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
        i += 1;
    }

    Diagram::Er(diagram)
}

// ---------------------------------------------------------------------------
// Comment masking (block `/' '/` and line `'`)
// ---------------------------------------------------------------------------

/// Mask every comment in `source` with spaces, in a single quote-aware,
/// byte-length-preserving pass.
///
/// Block comments (`/' ... '/`, possibly multi-line) and line comments (`'`
/// to end-of-line) are replaced by spaces; newlines are kept in place. Every
/// input byte maps to exactly one output byte, so byte offsets — and therefore
/// diagnostic spans — line up with the original source.
///
/// Quote-aware: a `'`, `/'`, or `'/` inside a `"..."` string is literal text
/// and never starts/ends a comment; a `"` inside a comment is ignored. This is
/// what prevents a `/'` inside a line comment or quoted display name from
/// opening a phantom block comment and silently swallowing later input.
///
/// Returns the masked string and, if the source ended while still inside a
/// block comment, the span of the unterminated `/'` opener so the caller can
/// emit a diagnostic instead of silently swallowing everything after it.
fn mask_comments(source: &str) -> (String, Option<Range<usize>>) {
    #[derive(PartialEq)]
    enum Mode {
        Normal,
        Quote,
        Line,
        Block,
    }

    let mut out = String::with_capacity(source.len());
    let mut mode = Mode::Normal;
    // Byte offset of the `/'` that opened the block comment we are currently in.
    let mut block_open: Option<usize> = None;
    let mut chars = source.char_indices().peekable();

    // Push `n` spaces (used to mask an interior char while preserving byte length).
    let push_spaces = |out: &mut String, n: usize| {
        for _ in 0..n {
            out.push(' ');
        }
    };

    while let Some((i, c)) = chars.next() {
        match mode {
            Mode::Normal => {
                if c == '"' {
                    out.push('"');
                    mode = Mode::Quote;
                } else if c == '/' && matches!(chars.peek(), Some(&(_, '\''))) {
                    chars.next(); // consume the '\'' of the `/'` opener (both ASCII)
                    out.push(' ');
                    out.push(' ');
                    mode = Mode::Block;
                    block_open = Some(i);
                } else if c == '\'' {
                    out.push(' ');
                    mode = Mode::Line;
                } else {
                    out.push(c);
                }
            }
            Mode::Quote => {
                out.push(c);
                // PlantUML string literals are single-line; recover at newline so an
                // unterminated quote can't swallow the rest of the file.
                if c == '"' || c == '\n' {
                    mode = Mode::Normal;
                }
            }
            Mode::Line => {
                if c == '\n' {
                    out.push('\n');
                    mode = Mode::Normal;
                } else {
                    push_spaces(&mut out, c.len_utf8());
                }
            }
            Mode::Block => {
                if c == '\'' && matches!(chars.peek(), Some(&(_, '/'))) {
                    chars.next(); // consume the '/' of the `'/` closer (both ASCII)
                    out.push(' ');
                    out.push(' ');
                    mode = Mode::Normal;
                    block_open = None;
                } else if c == '\n' {
                    out.push('\n');
                } else {
                    push_spaces(&mut out, c.len_utf8());
                }
            }
        }
    }

    // If we ended inside a block comment, report the opener so nothing is dropped
    // without a diagnostic.
    let unterminated = if mode == Mode::Block {
        block_open.map(|s| s..source.len())
    } else {
        None
    };

    (out, unterminated)
}

/// Collect non-blank lines from the comment-masked source, with their byte
/// offsets into the original source (byte length is preserved by masking).
fn logical_lines(masked: &str) -> Vec<(usize, String)> {
    let mut result = Vec::new();
    let mut offset = 0usize;

    for raw in masked.split('\n') {
        if !raw.trim().is_empty() {
            result.push((offset, raw.to_string()));
        }
        offset += raw.len() + 1; // +1 for the '\n' consumed by split
    }

    result
}

// ---------------------------------------------------------------------------
// Participant declaration parser
// ---------------------------------------------------------------------------

/// Keywords that introduce participant declarations (in addition to `participant` and `actor`).
const PARTICIPANT_KEYWORDS: &[&str] = &[
    "participant",
    "actor",
    "boundary",
    "control",
    "entity",
    "database",
    "collections",
    "queue",
];

/// Map a PlantUML participant keyword to a [`ParticipantKind`].
fn keyword_to_participant_kind(kw: &str) -> ParticipantKind {
    match kw.to_ascii_lowercase().as_str() {
        "actor" => ParticipantKind::Actor,
        "boundary" => ParticipantKind::Boundary,
        "control" => ParticipantKind::Control,
        "entity" => ParticipantKind::Entity,
        "database" => ParticipantKind::Database,
        "collections" => ParticipantKind::Collections,
        "queue" => ParticipantKind::Queue,
        _ => ParticipantKind::Default,
    }
}

/// Try to parse a participant/actor/icon-variant declaration.
///
/// Returns `Some((id, Option<label>, ParticipantKind))` on success, `None` if not a participant line.
/// - `participant Name` → id="Name", label=None (label defaults to id)
/// - `participant Name as Alias` → id="Alias", label=Some("Name")
/// - `participant "Quoted Name" as Alias` → id="Alias", label=Some("Quoted Name")
fn try_parse_participant_decl(line: &str) -> Option<(String, Option<String>, ParticipantKind)> {
    let (kw, rest) = split_keyword(line)?;
    if !PARTICIPANT_KEYWORDS
        .iter()
        .any(|&k| k.eq_ignore_ascii_case(kw))
    {
        return None;
    }
    let kind = keyword_to_participant_kind(kw);
    let rest = rest.trim();

    // Check for quoted display name: `"Quoted Name" as Alias`.
    if let Some(after_open_quote) = rest.strip_prefix('"') {
        // Find the closing quote.
        let end_quote = after_open_quote.find('"')?;
        let display_name = after_open_quote[..end_quote].to_string();
        let after_quote = after_open_quote[end_quote + 1..].trim(); // skip closing "
        if after_quote.is_empty() {
            // No `as` — use the quoted name as both id and label.
            return Some((display_name.clone(), Some(display_name), kind));
        }
        if let Some(alias) = strip_keyword_boundary_ci(after_quote, "as") {
            let alias = alias.trim().to_string();
            // Alias must be a single bare token; anything else is malformed.
            if !is_single_token(&alias) {
                return None;
            }
            return Some((alias, Some(display_name), kind));
        }
        // Trailing tokens that are not an `as` clause — reject rather than
        // silently discard them.
        return None;
    }

    // Unquoted name: `Name` or `Name as Alias`.
    if let Some(as_idx) = find_as_boundary(rest) {
        // `Name as Alias`
        let name = rest[..as_idx].trim().to_string();
        let alias = rest[as_idx + 4..].trim().to_string(); // " as " is 4 chars
                                                           // Both parts must be single bare tokens (unquoted names cannot contain
                                                           // whitespace); otherwise this is a malformed declaration, not a phantom
                                                           // participant.
        if !is_single_token(&name) || !is_single_token(&alias) {
            return None;
        }
        Some((alias, Some(name), kind))
    } else {
        // `Name` — id and label are both the name. A trailing bare `as` or an
        // interior space (e.g. `participant Foo as`, `participant Foo Bar`)
        // makes this malformed; reject so the caller reports an error.
        let name = rest.to_string();
        if !is_single_token(&name) {
            return None;
        }
        Some((name, None, kind))
    }
}

/// A bare (unquoted) PlantUML name/alias: non-empty and free of whitespace.
fn is_single_token(s: &str) -> bool {
    !s.is_empty() && !s.chars().any(|c| c.is_whitespace())
}

/// Split off the first whitespace-delimited keyword from `line`.
/// Returns `(keyword, rest_after_whitespace)` or None if line is empty.
fn split_keyword(line: &str) -> Option<(&str, &str)> {
    let line = line.trim_start();
    if line.is_empty() {
        return None;
    }
    let end = line
        .char_indices()
        .find(|&(_, c)| c.is_ascii_whitespace())
        .map(|(i, _)| i)
        .unwrap_or(line.len());
    let kw = &line[..end];
    let rest = line[end..].trim_start();
    Some((kw, rest))
}

/// Strip `as ` keyword prefix with word-boundary check (case-insensitive).
/// Returns the text after `as` if matched, None otherwise.
///
/// Uses `str::get` for the prefix slice so a non-ASCII leading character (whose
/// byte length straddles `keyword.len()`) returns `None` rather than panicking.
fn strip_keyword_boundary_ci<'a>(s: &'a str, keyword: &str) -> Option<&'a str> {
    match s.get(..keyword.len()) {
        Some(prefix)
            if prefix.eq_ignore_ascii_case(keyword)
                && (s.len() == keyword.len()
                    || s[keyword.len()..].starts_with(|c: char| c.is_ascii_whitespace())) =>
        {
            Some(&s[keyword.len()..])
        }
        _ => None,
    }
}

/// Find ` as ` in `s` with word-boundary semantics, returning the index of ` as `.
fn find_as_boundary(s: &str) -> Option<usize> {
    // Look for " as " (with leading space) — this ensures word boundary.
    let needle = " as ";
    let idx = s.find(needle)?;
    Some(idx)
}

// ---------------------------------------------------------------------------
// Message parser
// ---------------------------------------------------------------------------

/// (from, to, label, line_style, head, tail)
type SeqMsgResult = (
    String,
    String,
    Option<String>,
    LineStyle,
    MessageArrow,
    MessageArrow,
);

/// Try to parse a PlantUML sequence message line.
///
/// Arrow forms and their mapping:
/// | PlantUML | LineStyle | head / tail |
/// |----------|-----------|-------------|
/// | `->` | Solid | Filled |
/// | `-->` | Dashed | Filled |
/// | `->>` | Solid | Open (thin/async arrowhead) |
/// | `-->>` | Dashed | Open |
/// | `->x` / `-->x` | Solid / Dashed | Cross (lost message) |
/// | `->o` / `-->o` | Solid / Dashed | Circle (found message) |
/// | `<->` / `<-->` | Solid / Dashed | head Filled + tail Filled |
///
/// Format: `From ARROW To : label` or `From ARROW To`
/// Participants may be bare identifiers.
///
/// Returns `None` if the line does not look like a message.
/// Returns `Some(Err(msg))` if it looks like a message but has a parse error.
/// Returns `Some(Ok(...))` on success.
fn try_parse_plantuml_message(line: &str) -> Option<Result<SeqMsgResult, String>> {
    // Check for unsupported colored arrows: -[#color]>
    if line.contains("-[") {
        return Some(Err(
            "unsupported: colored arrows (`-[#color]>`) are not supported".to_string(),
        ));
    }

    // Arrow forms: longest first to avoid prefix ambiguity (`-->>` before
    // `-->`, `->>` before `->`, `<-->` before `-->`, `->x`/`->o` before `->`).
    #[allow(clippy::type_complexity)]
    let arrow_forms: &[(&str, LineStyle, MessageArrow, MessageArrow)] = &[
        (
            "<-->",
            LineStyle::Dashed,
            MessageArrow::Filled,
            MessageArrow::Filled,
        ),
        (
            "-->>",
            LineStyle::Dashed,
            MessageArrow::Open,
            MessageArrow::None,
        ),
        (
            "-->x",
            LineStyle::Dashed,
            MessageArrow::Cross,
            MessageArrow::None,
        ),
        (
            "-->o",
            LineStyle::Dashed,
            MessageArrow::Circle,
            MessageArrow::None,
        ),
        (
            "-->",
            LineStyle::Dashed,
            MessageArrow::Filled,
            MessageArrow::None,
        ),
        (
            "->>",
            LineStyle::Solid,
            MessageArrow::Open,
            MessageArrow::None,
        ),
        (
            "->x",
            LineStyle::Solid,
            MessageArrow::Cross,
            MessageArrow::None,
        ),
        (
            "->o",
            LineStyle::Solid,
            MessageArrow::Circle,
            MessageArrow::None,
        ),
        (
            "<->",
            LineStyle::Solid,
            MessageArrow::Filled,
            MessageArrow::Filled,
        ),
        (
            "->",
            LineStyle::Solid,
            MessageArrow::Filled,
            MessageArrow::None,
        ),
    ];

    for &(arrow_str, line_style, head, tail) in arrow_forms {
        if let Some(idx) = line.find(arrow_str) {
            let from_part = line[..idx].trim();
            let after = &line[idx + arrow_str.len()..];

            // `->x` / `->o` end in a letter: require a whitespace boundary so
            // a target id starting with `x`/`o` (e.g. `A ->oscar`) is parsed
            // as `->` + `oscar`, not as a found-message marker.
            if arrow_str.ends_with(|c: char| c.is_ascii_alphanumeric())
                && !after.starts_with(|c: char| c.is_ascii_whitespace())
            {
                continue;
            }

            // from_part must be a valid participant id.
            if from_part.is_empty() || !is_valid_participant_id(from_part) {
                continue;
            }
            let from = from_part.to_string();

            // Parse `To : label` or just `To`.
            let after = after.trim_start();
            let (to, label) = if let Some(colon_idx) = after.find(':') {
                let to_part = after[..colon_idx].trim();
                if to_part.is_empty() || !is_valid_participant_id(to_part) {
                    continue;
                }
                let to = to_part.to_string();
                let lbl = after[colon_idx + 1..].trim().to_string();
                let label = if lbl.is_empty() { None } else { Some(lbl) };
                (to, label)
            } else {
                let to_part = after.trim();
                if to_part.is_empty() || !is_valid_participant_id(to_part) {
                    continue;
                }
                (to_part.to_string(), None)
            };

            return Some(Ok((from, to, label, line_style, head, tail)));
        }
    }

    None
}

/// Check if `s` is a valid PlantUML participant identifier (bare, unquoted).
/// Accepts alphanumeric, underscore, and Unicode letters (for Japanese names etc.).
fn is_valid_participant_id(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    // Allow alphanumeric, underscore, hyphen, and any non-ASCII Unicode letters.
    s.chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use kozue_ir::{EndMarker, LineStyle, SequenceItem};

    #[test]
    fn parse_document_preserves_optional_plantuml_name() {
        let named = parse_document("@startuml checkout flow\n@enduml\n").unwrap();
        assert_eq!(named.metadata.name.as_deref(), Some("checkout flow"));
        assert!(named.extensions.is_empty());

        let unnamed = parse_document("@startuml\n@enduml\n").unwrap();
        assert_eq!(unnamed.metadata.name, None);
        assert_eq!(
            parse("@startuml\n@enduml\n").unwrap(),
            unnamed.into_diagram()
        );
    }

    fn parse_ok(src: &str) -> SequenceDiagram {
        match parse(src).expect("should parse without errors") {
            Diagram::Sequence(s) => s,
            _ => panic!("expected Sequence diagram"),
        }
    }

    fn parse_err(src: &str) -> Vec<Diagnostic> {
        parse(src).expect_err("should return errors")
    }

    // -----------------------------------------------------------------------
    // Basic parsing
    // -----------------------------------------------------------------------

    #[test]
    fn basic_two_participant_sequence() {
        let src = "@startuml\nparticipant Alice\nparticipant Bob\nAlice -> Bob : hello\n@enduml\n";
        let s = parse_ok(src);
        assert_eq!(s.participants.len(), 2);
        assert!(s.participants.contains_key("Alice"));
        assert!(s.participants.contains_key("Bob"));
        assert_eq!(s.items.len(), 1);
        let SequenceItem::Message(ref m) = s.items[0] else {
            panic!()
        };
        assert_eq!(m.from.as_str(), "Alice");
        assert_eq!(m.to.as_str(), "Bob");
        assert_eq!(m.label.as_deref(), Some("hello"));
        assert_eq!(m.line, LineStyle::Solid);
        assert_eq!(m.head, MessageArrow::Filled);
        assert_eq!(m.tail, MessageArrow::None);
    }

    #[test]
    fn as_alias_sets_id_and_label() {
        let src = "@startuml\nparticipant Alice as A\nparticipant Bob as B\nA -> B : hi\n@enduml\n";
        let s = parse_ok(src);
        assert_eq!(s.participants.len(), 2);
        // id is the alias
        assert!(s.participants.contains_key("A"));
        assert_eq!(s.participants["A"].label, "Alice");
        assert!(s.participants.contains_key("B"));
        assert_eq!(s.participants["B"].label, "Bob");
    }

    #[test]
    fn quoted_display_name_with_alias() {
        let src =
            "@startuml\nparticipant \"Web Browser\" as WB\nparticipant Server\nWB -> Server : GET /\n@enduml\n";
        let s = parse_ok(src);
        assert!(s.participants.contains_key("WB"));
        assert_eq!(s.participants["WB"].label, "Web Browser");
        assert!(s.participants.contains_key("Server"));
    }

    #[test]
    fn auto_declare_participants_on_first_use() {
        // No explicit participant declarations — auto-declared via message.
        let src = "@startuml\nA -> B : test\n@enduml\n";
        let s = parse_ok(src);
        assert_eq!(s.participants.len(), 2);
        assert!(s.participants.contains_key("A"));
        assert!(s.participants.contains_key("B"));
        assert_eq!(s.participants["A"].label, "A");
        assert_eq!(s.participants["B"].label, "B");
    }

    #[test]
    fn self_message() {
        let src = "@startuml\nparticipant A\nA -> A : think\n@enduml\n";
        let s = parse_ok(src);
        assert_eq!(s.items.len(), 1);
        let SequenceItem::Message(ref m) = s.items[0] else {
            panic!()
        };
        assert_eq!(m.from.as_str(), "A");
        assert_eq!(m.to.as_str(), "A");
        assert_eq!(m.label.as_deref(), Some("think"));
    }

    #[test]
    fn solid_arrow_maps_to_solid_filled_head() {
        let src = "@startuml\nA -> B : msg\n@enduml\n";
        let s = parse_ok(src);
        let SequenceItem::Message(ref m) = s.items[0] else {
            panic!()
        };
        assert_eq!(m.line, LineStyle::Solid);
        assert_eq!(m.head, MessageArrow::Filled);
        assert_eq!(m.tail, MessageArrow::None);
    }

    #[test]
    fn dashed_arrow_maps_to_dashed_filled_head() {
        let src = "@startuml\nA --> B : msg\n@enduml\n";
        let s = parse_ok(src);
        let SequenceItem::Message(ref m) = s.items[0] else {
            panic!()
        };
        assert_eq!(m.line, LineStyle::Dashed);
        assert_eq!(m.head, MessageArrow::Filled);
        assert_eq!(m.tail, MessageArrow::None);
    }

    #[test]
    fn thin_solid_arrow_maps_to_open_head() {
        // ->> maps to Solid + Open (async/thin arrowhead)
        let src = "@startuml\nA ->> B : msg\n@enduml\n";
        let s = parse_ok(src);
        let SequenceItem::Message(ref m) = s.items[0] else {
            panic!()
        };
        assert_eq!(m.line, LineStyle::Solid);
        assert_eq!(m.head, MessageArrow::Open);
        assert_eq!(m.tail, MessageArrow::None);
    }

    #[test]
    fn thin_dashed_arrow_maps_to_open_head() {
        // -->> maps to Dashed + Open
        let src = "@startuml\nA -->> B : msg\n@enduml\n";
        let s = parse_ok(src);
        let SequenceItem::Message(ref m) = s.items[0] else {
            panic!()
        };
        assert_eq!(m.line, LineStyle::Dashed);
        assert_eq!(m.head, MessageArrow::Open);
        assert_eq!(m.tail, MessageArrow::None);
    }

    #[test]
    fn lost_found_and_bidirectional_arrows_map_to_heads() {
        let cases: &[(&str, LineStyle, MessageArrow, MessageArrow)] = &[
            (
                "A ->x B : m",
                LineStyle::Solid,
                MessageArrow::Cross,
                MessageArrow::None,
            ),
            (
                "A -->x B : m",
                LineStyle::Dashed,
                MessageArrow::Cross,
                MessageArrow::None,
            ),
            (
                "A ->o B : m",
                LineStyle::Solid,
                MessageArrow::Circle,
                MessageArrow::None,
            ),
            (
                "A -->o B : m",
                LineStyle::Dashed,
                MessageArrow::Circle,
                MessageArrow::None,
            ),
            (
                "A <-> B : m",
                LineStyle::Solid,
                MessageArrow::Filled,
                MessageArrow::Filled,
            ),
            (
                "A <--> B : m",
                LineStyle::Dashed,
                MessageArrow::Filled,
                MessageArrow::Filled,
            ),
        ];
        for &(line, style, head, tail) in cases {
            let src = format!("@startuml\n{line}\n@enduml\n");
            let s = parse_ok(&src);
            let SequenceItem::Message(ref m) = s.items[0] else {
                panic!()
            };
            assert_eq!(m.line, style, "{line}");
            assert_eq!(m.head, head, "{line}");
            assert_eq!(m.tail, tail, "{line}");
        }
    }

    #[test]
    fn found_marker_requires_word_boundary() {
        // `A ->oscar : m` is `->` + participant `oscar`, not a found message.
        let src = "@startuml\nA ->oscar : m\n@enduml\n";
        let s = parse_ok(src);
        let SequenceItem::Message(ref m) = s.items[0] else {
            panic!()
        };
        assert_eq!(m.to.as_str(), "oscar");
        assert_eq!(m.head, MessageArrow::Filled);
    }

    #[test]
    fn actor_keyword_works_like_participant() {
        let src = "@startuml\nactor User\nparticipant System\nUser -> System : login\n@enduml\n";
        let s = parse_ok(src);
        assert!(s.participants.contains_key("User"));
        assert_eq!(s.participants["User"].label, "User");
        assert_eq!(s.participants["User"].kind, ParticipantKind::Actor);
        assert_eq!(s.participants["System"].kind, ParticipantKind::Default);
    }

    #[test]
    fn actor_with_alias() {
        let src = "@startuml\nactor \"End User\" as U\nU -> System : click\n@enduml\n";
        let s = parse_ok(src);
        assert!(s.participants.contains_key("U"));
        assert_eq!(s.participants["U"].label, "End User");
    }

    #[test]
    fn icon_variant_keywords_map_to_participant_kinds() {
        let src = "@startuml\nboundary FE\ncontrol BE\nentity DB\ndatabase Store\ncollections Q\nqueue MQ\nFE -> BE : req\nBE -> DB : query\n@enduml\n";
        let s = parse_ok(src);
        assert_eq!(s.participants["FE"].kind, ParticipantKind::Boundary);
        assert_eq!(s.participants["BE"].kind, ParticipantKind::Control);
        assert_eq!(s.participants["DB"].kind, ParticipantKind::Entity);
        assert_eq!(s.participants["Store"].kind, ParticipantKind::Database);
        assert_eq!(s.participants["Q"].kind, ParticipantKind::Collections);
        assert_eq!(s.participants["MQ"].kind, ParticipantKind::Queue);
    }

    #[test]
    fn message_without_label() {
        let src = "@startuml\nA -> B\n@enduml\n";
        let s = parse_ok(src);
        let SequenceItem::Message(ref m) = s.items[0] else {
            panic!()
        };
        assert_eq!(m.label, None);
    }

    // -----------------------------------------------------------------------
    // Error cases
    // -----------------------------------------------------------------------

    #[test]
    fn missing_startuml_is_error() {
        let src = "participant A\nA -> B : hi\n@enduml\n";
        let errs = parse_err(src);
        assert!(
            errs.iter().any(|e| e.message.contains("@startuml")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn missing_enduml_is_error() {
        let src = "@startuml\nA -> B : hi\n";
        let errs = parse_err(src);
        assert!(
            errs.iter().any(|e| e.message.contains("@enduml")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn startmindmap_is_unsupported() {
        let src = "@startmindmap\n* root\n@endmindmap\n";
        let errs = parse_err(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unsupported") && e.message.contains("mindmap")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn startgantt_is_unsupported() {
        let src = "@startgantt\nProject starts 2024-01-01\n@endgantt\n";
        let errs = parse_err(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unsupported") && e.message.contains("gantt")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn startjson_is_unsupported() {
        let src = "@startjson\n{}\n@endjson\n";
        let errs = parse_err(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unsupported") || e.message.contains("json")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn preprocessor_include_is_unsupported() {
        let src = "@startuml\n!include other.puml\nA -> B : hi\n@enduml\n";
        let errs = parse_err(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unsupported") && e.message.contains("preprocessor")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn preprocessor_define_is_unsupported() {
        let src = "@startuml\n!define FOO bar\nA -> B : FOO\n@enduml\n";
        let errs = parse_err(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unsupported") && e.message.contains("preprocessor")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn note_is_unsupported() {
        let src = "@startuml\nparticipant A\nnote over A: some text\nA -> A : ok\n@enduml\n";
        let errs = parse_err(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unsupported")
                    && e.message.to_lowercase().contains("note")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn alt_is_unsupported() {
        let src = "@startuml\nA -> B : req\nalt success\n  B --> A : ok\nend\n@enduml\n";
        let errs = parse_err(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unsupported") && e.message.contains("alt")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn loop_is_unsupported() {
        let src = "@startuml\nloop every second\n  A -> A : tick\nend\n@enduml\n";
        let errs = parse_err(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unsupported") && e.message.contains("loop")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn end_silently_skipped() {
        // `end` alone on a line should be silently skipped (not cause an error).
        // Body: participant A, then an implicit alt-end that got orphaned.
        let src = "@startuml\nparticipant A\nend\nA -> A : ok\n@enduml\n";
        // Should parse successfully (no error for bare `end`).
        let s = parse_ok(src);
        assert_eq!(s.items.len(), 1);
    }

    #[test]
    fn line_comment_is_stripped() {
        // `'` comment should be stripped; the message should parse.
        let src = "@startuml\n' this is a comment\nparticipant A\nparticipant B\nA -> B : msg ' inline comment\n@enduml\n";
        let s = parse_ok(src);
        assert_eq!(s.items.len(), 1);
        let SequenceItem::Message(ref m) = s.items[0] else {
            panic!()
        };
        // Label should contain "msg" (trailing space before comment is preserved, comment stripped).
        let label = m.label.as_deref().unwrap_or("");
        assert!(
            label.contains("msg"),
            "label should contain 'msg', got: {label:?}"
        );
        assert!(
            !label.contains("inline comment"),
            "label must not contain comment text"
        );
    }

    #[test]
    fn block_comment_is_stripped() {
        let src = "@startuml\n/' this is a block comment '/\nA -> B : hello\n@enduml\n";
        let s = parse_ok(src);
        assert_eq!(s.items.len(), 1);
    }

    #[test]
    fn multiline_block_comment_is_stripped() {
        let src =
            "@startuml\n/'\nline 1 of comment\nline 2 of comment\n'/\nA -> B : hello\n@enduml\n";
        let s = parse_ok(src);
        assert_eq!(s.items.len(), 1);
    }

    #[test]
    fn three_participants_with_solid_and_dashed() {
        let src = "@startuml\nparticipant Alice as A\nparticipant Bob as B\nparticipant Carol as C\nA -> B : request\nB --> A : response\nA -> C : notify\n@enduml\n";
        let s = parse_ok(src);
        assert_eq!(s.participants.len(), 3);
        assert_eq!(s.items.len(), 3);
        let SequenceItem::Message(ref m0) = s.items[0] else {
            panic!()
        };
        assert_eq!(m0.line, LineStyle::Solid);
        let SequenceItem::Message(ref m1) = s.items[1] else {
            panic!()
        };
        assert_eq!(m1.line, LineStyle::Dashed);
    }

    #[test]
    fn par_is_unsupported_not_confused_with_participant() {
        // `par` should be unsupported but `participant` should still work.
        let src = "@startuml\nparticipant Alice\npar\nAlice -> Alice : ok\nend\n@enduml\n";
        let errs = parse_err(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unsupported") && e.message.contains("par")),
            "`par` should be unsupported, got: {errs:?}"
        );
    }

    #[test]
    fn startuml_with_name_is_accepted() {
        // @startuml SomeName — name is accepted and ignored.
        let src = "@startuml MyDiagram\nA -> B : hi\n@enduml\n";
        let s = parse_ok(src);
        assert_eq!(s.items.len(), 1);
    }

    #[test]
    fn japanese_labels_in_messages() {
        let src = "@startuml\nparticipant クライアント as C\nparticipant サーバ as S\nC -> S : ログイン\nS --> C : 成功\n@enduml\n";
        let s = parse_ok(src);
        assert_eq!(s.participants.len(), 2);
        assert_eq!(s.participants["C"].label, "クライアント");
        assert_eq!(s.items.len(), 2);
        let SequenceItem::Message(ref m0) = s.items[0] else {
            panic!()
        };
        assert_eq!(m0.label.as_deref(), Some("ログイン"));
    }

    // -----------------------------------------------------------------------
    // Regression: comment masking must not silently drop input (BLOCKER #1)
    // -----------------------------------------------------------------------

    #[test]
    fn block_open_inside_line_comment_does_not_swallow_input() {
        // A `/'` sitting inside a `'` line comment must NOT open a block comment
        // and eat the following real message.
        let src = "@startuml\n' TODO: convert to /' block\nSECRET -> LEAK : must not vanish\n' done '/\nA -> B : visible\n@enduml\n";
        let s = parse_ok(src);
        // Both real messages must survive.
        assert_eq!(s.items.len(), 2, "no message may be silently dropped");
        let froms: Vec<&str> = s
            .items
            .iter()
            .filter_map(|it| match it {
                SequenceItem::Message(m) => Some(m.from.as_str()),
                _ => None,
            })
            .collect();
        assert!(froms.contains(&"SECRET"), "got: {froms:?}");
        assert!(froms.contains(&"A"), "got: {froms:?}");
    }

    #[test]
    fn block_open_inside_quoted_name_does_not_swallow_input() {
        // `/'` inside a quoted display name is literal, not a block opener.
        let src = "@startuml\nparticipant \"a /' b\" as X\nX -> Y : survives\n@enduml\n";
        let s = parse_ok(src);
        assert_eq!(s.participants["X"].label, "a /' b");
        assert_eq!(s.items.len(), 1, "message after quoted /' must survive");
    }

    // -----------------------------------------------------------------------
    // Regression: diagnostic spans stay aligned after a block comment (BLOCKER #2)
    // -----------------------------------------------------------------------

    #[test]
    fn unterminated_block_comment_is_error() {
        // A `/'` that is never closed masks to EOF; this must be diagnosed, not
        // silently swallowed (including the swallowed message and @enduml).
        let src = "@startuml\nA -> B : hi\n/' oops never closed\nC -> D : lost\n@enduml\n";
        let errs = parse_err(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unterminated block comment")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn error_span_is_correct_after_block_comment() {
        let src = "@startuml\n/' a block comment here '/\nnote over A : unsupported\n@enduml\n";
        let errs = parse_err(src);
        let note_err = errs
            .iter()
            .find(|e| e.message.contains("note"))
            .expect("expected a note error");
        // The span must underline the actual `note` occurrence in the ORIGINAL source.
        assert_eq!(
            &src[note_err.span.clone()],
            "note over A : unsupported",
            "span must map to the note line, not shift due to the block comment"
        );
    }

    // -----------------------------------------------------------------------
    // Regression: malformed participant decls error instead of misparsing
    // -----------------------------------------------------------------------

    #[test]
    fn dangling_as_is_error_not_phantom_participant() {
        let src = "@startuml\nparticipant Foo as\nFoo -> Bar : hi\n@enduml\n";
        let errs = parse_err(src);
        assert!(
            errs.iter().any(|e| e.message.contains("unrecognised")),
            "trailing `as` must be an error, got: {errs:?}"
        );
    }

    #[test]
    fn duplicate_participant_id_is_error() {
        let src = "@startuml\nparticipant Alice as X\nparticipant Bob as X\nX -> X : hi\n@enduml\n";
        let errs = parse_err(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("duplicate participant")),
            "duplicate id must be an error, got: {errs:?}"
        );
    }

    #[test]
    fn multiple_errors_are_all_collected() {
        // Both note and alt should produce errors — all collected before returning.
        let src = "@startuml\nnote over A: text\nalt success\nA -> B : hi\nend\n@enduml\n";
        let errs = parse_err(src);
        assert!(errs.len() >= 2, "expected multiple errors, got: {errs:?}");
    }

    // -----------------------------------------------------------------------
    // State diagram tests
    // -----------------------------------------------------------------------

    fn parse_state_ok(src: &str) -> StateDiagram {
        match parse(src).expect("should parse without errors") {
            Diagram::State(s) => s,
            other => panic!("expected State diagram, got {other:?}"),
        }
    }

    #[test]
    fn state_basic_flow() {
        let src = "@startuml\n[*] --> Idle\nIdle --> Running : start\nRunning --> [*]\n@enduml\n";
        let s = parse_state_ok(src);
        assert!(s.states.contains_key("Idle"));
        assert!(s.states.contains_key("Running"));
        assert_eq!(s.transitions.len(), 3);
        assert_eq!(s.transitions[0].from, Endpoint::Initial);
        assert_eq!(s.transitions[0].to, Endpoint::State("Idle".into()));
        assert_eq!(s.transitions[1].label.as_deref(), Some("start"));
        assert_eq!(s.transitions[2].to, Endpoint::Final);
    }

    #[test]
    fn state_solid_arrow_also_a_transition() {
        // In a state diagram (signalled by `[*]`), `->` is a transition too.
        let src = "@startuml\n[*] -> A\nA -> B\n@enduml\n";
        let s = parse_state_ok(src);
        assert_eq!(s.transitions.len(), 2);
        assert!(s.states.contains_key("A") && s.states.contains_key("B"));
    }

    #[test]
    fn state_explicit_declaration_with_alias() {
        let src = "@startuml\nstate \"Long Name\" as s1\n[*] --> s1\n@enduml\n";
        let s = parse_state_ok(src);
        assert_eq!(s.states.get("s1").unwrap().label, "Long Name");
    }

    #[test]
    fn state_bare_declaration() {
        let src = "@startuml\nstate Alone\n[*] --> Alone\n@enduml\n";
        let s = parse_state_ok(src);
        assert_eq!(s.states.get("Alone").unwrap().label, "Alone");
    }

    #[test]
    fn state_without_pseudostate_but_with_state_keyword() {
        // No `[*]`, but a `state` keyword still signals a state diagram.
        let src = "@startuml\nstate A\nA --> B\n@enduml\n";
        let s = parse_state_ok(src);
        assert_eq!(s.transitions.len(), 1);
    }

    #[test]
    fn plain_dashed_message_stays_a_sequence() {
        // No state signal at all → the `A --> B` reads as a sequence message.
        let src = "@startuml\nA --> B : hi\n@enduml\n";
        match parse(src).expect("should parse") {
            Diagram::Sequence(_) => {}
            other => panic!("expected Sequence, got {other:?}"),
        }
    }

    #[test]
    fn state_initial_to_final_is_error() {
        let src = "@startuml\n[*] --> [*]\n@enduml\n";
        let errs = parse_err(src);
        assert!(
            errs.iter().any(|e| e.message.contains("[*] --> [*]")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn state_composite_is_unsupported() {
        let src = "@startuml\nstate Outer {\n[*] --> Inner\n}\n@enduml\n";
        let errs = parse_err(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unsupported") && e.message.contains("composite")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn state_fork_is_unsupported() {
        let src = "@startuml\nstate fork_state <<fork>>\n[*] --> fork_state\n@enduml\n";
        let errs = parse_err(src);
        assert!(
            errs.iter().any(|e| e.message.contains("unsupported")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn state_note_is_unsupported() {
        let src = "@startuml\n[*] --> A\nnote right of A : hi\n@enduml\n";
        let errs = parse_err(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unsupported") && e.message.contains("note")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn state_description_assignment_is_unsupported() {
        let src = "@startuml\nstate A\nA : doing work\n@enduml\n";
        let errs = parse_err(src);
        assert!(!errs.is_empty(), "got: {errs:?}");
    }

    #[test]
    fn state_duplicate_declaration_is_error() {
        let src = "@startuml\nstate \"A\" as x\nstate \"B\" as x\n[*] --> x\n@enduml\n";
        let errs = parse_err(src);
        assert!(
            errs.iter().any(|e| e.message.contains("duplicate")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn state_transition_label_containing_arrow() {
        // A label containing `-->` must not be mistaken for the transition arrow.
        let src = "@startuml\n[*] -> A\nA -> B : x --> y\n@enduml\n";
        let s = parse_state_ok(src);
        assert_eq!(s.transitions.len(), 2);
        assert_eq!(s.transitions[1].from, Endpoint::State("A".into()));
        assert_eq!(s.transitions[1].to, Endpoint::State("B".into()));
        assert_eq!(s.transitions[1].label.as_deref(), Some("x --> y"));
    }

    #[test]
    fn state_non_ascii_after_alias_does_not_panic() {
        // Non-ASCII trailing token after a quoted state must diagnose, not panic.
        let src = "@startuml\nstate \"名前\" あ\n[*] --> x\n@enduml\n";
        let errs = parse_err(src);
        assert!(!errs.is_empty());
    }

    #[test]
    fn pseudostate_marker_inside_message_label_stays_sequence() {
        // `[*]` appears only inside a sequence message label — it must NOT be read
        // as a state-diagram signal.
        let src = "@startuml\nA -> B : send [*] token\n@enduml\n";
        match parse(src).expect("should parse") {
            Diagram::Sequence(s) => {
                assert_eq!(s.items.len(), 1);
            }
            other => panic!("expected Sequence, got {other:?}"),
        }
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
        let src = "@startuml\nclass Animal {\n  +String name\n  +makeSound() void\n}\nclass Dog {\n  +bark() void\n}\nDog --|> Animal\n@enduml\n";
        let d = parse(src).expect("should parse");
        let c = class_diagram(&d);
        assert_eq!(c.classes.len(), 2);
        assert!(c.classes["Animal"].attributes[0].contains("name"));
        assert_eq!(c.classes["Animal"].methods[0], "+makeSound(): void");
        assert_eq!(c.relations.len(), 1);
        assert_eq!(c.relations[0].from.as_str(), "Dog");
        assert_eq!(c.relations[0].to.as_str(), "Animal");
        assert_eq!(c.relations[0].from_marker, EndMarker::None);
        assert_eq!(c.relations[0].to_marker, EndMarker::HollowTriangle);
    }

    #[test]
    fn class_native_inheritance_direction() {
        let src = "@startuml\nDog <|-- Animal\n@enduml\n";
        let d = parse(src).expect("should parse");
        let c = class_diagram(&d);
        assert_eq!(c.relations[0].from_marker, EndMarker::HollowTriangle);
        assert_eq!(c.relations[0].to_marker, EndMarker::None);
    }

    /// Helper: parse a single-relation class body and return the relation.
    ///
    /// A `class` declaration is included so the body is unambiguously a class
    /// diagram even for marker-less connectors (`-->`, `--`, `..`), which on
    /// their own are intentionally read as a sequence (see the anti-ambiguity
    /// rule documented in `features.rs`). Class context is what makes every
    /// connector direction resolve as a class relation.
    fn class_one_relation(rel_line: &str) -> kozue_ir::ClassRelation {
        let src = format!("@startuml\nclass A\nclass B\n{rel_line}\n@enduml\n");
        let d = parse(&src).unwrap_or_else(|e| panic!("`{rel_line}` should parse: {e:?}"));
        let c = class_diagram(&d);
        assert_eq!(c.relations.len(), 1, "`{rel_line}` produced != 1 relation");
        c.relations[0].clone()
    }

    #[test]
    fn class_all_connector_directions_are_accepted() {
        use kozue_ir::EndMarker::*;
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
    fn class_interface_and_abstract_keywords() {
        let src = "@startuml\ninterface Shape\nabstract class Base\nShape <|.. Base\n@enduml\n";
        let d = parse(src).expect("should parse");
        let c = class_diagram(&d);
        assert_eq!(c.classes["Shape"].stereotype.as_deref(), Some("interface"));
        assert_eq!(c.classes["Base"].stereotype.as_deref(), Some("abstract"));
        assert_eq!(c.relations[0].line, LineStyle::Dashed);
    }

    #[test]
    fn class_generics_are_unsupported() {
        let src = "@startuml\nclass Box~T~\n@enduml\n";
        let errs = parse_err(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("unsupported") && e.message.contains("generic")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn class_relation_multiplicity() {
        // F4: `A "1" -- "*" B` — multiplicity next to each class.
        let src = "@startuml\nclass A\nclass B\nA \"1\" -- \"*\" B : owns\n@enduml\n";
        let d = parse(src).expect("should parse");
        let c = class_diagram(&d);
        let r = &c.relations[0];
        assert_eq!(r.from.as_str(), "A");
        assert_eq!(r.to.as_str(), "B");
        assert_eq!(r.from_mult.as_deref(), Some("1"));
        assert_eq!(r.to_mult.as_deref(), Some("*"));
        assert_eq!(r.label.as_deref(), Some("owns"));
    }

    #[test]
    fn class_single_line_inline_block() {
        // F3: `{` and `}` on the same line, members split by `;`.
        let src = "@startuml\nclass Point { +x : int; +y : int; +dist() : float }\n@enduml\n";
        let d = parse(src).expect("should parse");
        let c = class_diagram(&d);
        assert_eq!(c.classes["Point"].attributes, vec!["+x: int", "+y: int"]);
        assert_eq!(c.classes["Point"].methods, vec!["+dist(): float"]);
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
    fn er_entity_block_and_relation() {
        let src = "@startuml\nentity Customer {\n  * id : int\n  name : string\n}\nCustomer ||--o{ Order : places\n@enduml\n";
        let d = parse(src).expect("should parse");
        let e = er_diagram(&d);
        let entity = &e.entities["Customer"];
        assert_eq!(entity.attributes[0].name, "id");
        assert_eq!(entity.attributes[0].keys, vec!["PK".to_string()]);
        assert_eq!(entity.attributes[1].name, "name");
        assert!(entity.attributes[1].keys.is_empty());
        assert_eq!(e.relations[0].from_marker, EndMarker::ErOne);
        assert_eq!(e.relations[0].to_marker, EndMarker::ErZeroOrMany);
    }

    #[test]
    fn er_plain_dash_relation_has_no_markers() {
        let src = "@startuml\nentity A {\n  * id : int\n}\nentity B {\n  * id : int\n}\nA -- B : rel\n@enduml\n";
        let d = parse(src).expect("should parse");
        let e = er_diagram(&d);
        assert_eq!(e.relations[0].from_marker, EndMarker::None);
        assert_eq!(e.relations[0].to_marker, EndMarker::None);
    }

    #[test]
    fn er_single_line_inline_entity_block() {
        // F3: inline `entity Foo { a; b }` with `;`-separated attributes.
        let src = "@startuml\nentity Order { * id : int; customer_id : int }\nOrder ||--|| Customer\n@enduml\n";
        let d = parse(src).expect("should parse");
        let e = er_diagram(&d);
        let order = &e.entities["Order"];
        assert_eq!(order.attributes.len(), 2);
        assert_eq!(order.attributes[0].name, "id");
        assert_eq!(order.attributes[0].keys, vec!["PK".to_string()]);
        assert_eq!(order.attributes[1].name, "customer_id");
    }

    #[test]
    fn entity_without_braces_stays_a_sequence_participant() {
        // Regression: `entity DB` (no `{`) must not be misdetected as ER.
        let src = "@startuml\nentity DB\nA -> DB : query\n@enduml\n";
        let d = parse(src).expect("should parse");
        match d {
            Diagram::Sequence(_) => {}
            other => panic!("expected Sequence, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // never-silently-misparse: ambiguous body detection
    // -----------------------------------------------------------------------

    #[test]
    fn ambiguous_class_and_state_body_is_explicit_error() {
        // `class Foo` (class signal) combined with `[*] --> Foo` (state signal)
        // in the same body must NOT be silently guessed as one or the other.
        let src = "@startuml\nclass Foo\n[*] --> Foo\n@enduml\n";
        let errs = parse_err(src);
        assert!(
            errs.iter()
                .any(|e| e.message.contains("ambiguous") && e.message.contains("class")),
            "got: {errs:?}"
        );
    }

    #[test]
    fn ambiguous_class_and_er_body_is_explicit_error() {
        let src = "@startuml\nclass Foo\nentity Bar {\n  * id : int\n}\n@enduml\n";
        let errs = parse_err(src);
        assert!(
            errs.iter().any(|e| e.message.contains("ambiguous")),
            "got: {errs:?}"
        );
    }
}
