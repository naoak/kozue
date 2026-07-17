# kozue IR Extension Roadmap

## Purpose

Expand Mermaid / PlantUML coverage while maintaining the following requirements.

- Always produce byte-identical output from the same input
- Maintain the boundary between frontend → semantic IR → layout → renderer
- Do not force source-specific syntax into the shared IR
- Pass semantic information to draw.io / Excalidraw / PPTX without loss
- Explicitly manage compatibility of serialized IR

Do not flatten all diagrams into a one-size-fits-all graph. `Diagram` retains
domain-specific variants, and only concepts that are truly shareable — ID,
annotation, container, style, etc. — are extracted as common types.

## Overall Milestones

### M1: Versioned IR document

Status: **Implemented and committed**

- Add `IrDocument` and numeric wire version `IrSchemaVersion::V1`
- Add `CURRENT_IR_SCHEMA_VERSION` (V1 at M1, updated to V2 at M2)
- Add receptors for diagram name, title, description, and accessibility metadata
- Add deterministic `BTreeMap`-based namespaced `Extensions`
- Reject unknown schema versions on deserialize
- Mark newly introduced public types `#[non_exhaustive]` to allow future Rust API extension
- Add `parse_document` to native DSL / Mermaid / PlantUML
- Maintain existing `parse(source) -> Diagram` and existing `Diagram` wire representation
- Preserve diagram names for all 5 native DSL types and PlantUML
- Mermaid name is `None` in V1

V1 does not expose a mutation API for `Extensions`. Core semantics such as shapes,
relation kinds, notes, and fragments must not be stored in extensions.

### M2: Stable element identity and annotations

Status: **Implemented and committed**

- Introduce transparent newtype `ElementId`
- Migrate named elements in all 5 diagram types, ordered map keys, `from` / `to`,
  and `Endpoint::State` to `ElementId`
- Migrate corresponding `SemanticLayout` IDs and endpoints to `ElementId`
- Raw parser AST, diagnostics, Scene group names, and renderer-specific IDs remain as `String`
- Add `IrDocument.annotations: Vec<Annotation>` preserving declaration order
- Type annotation targets as diagram / single element / multiple elements
- Add shared annotation payloads: note, link, tooltip, stereotype, tag
- Add schema V2 and lossless upgrade of V1 documents to V2 with empty annotations
- Serialization at M2 is V2 (updated to V3 at M3a1, V4 at M3a2a-I, V5 at M3a2a-II,
  V6 at M3a2b, V7 at M3a3, V8 at M3a4); reject unknown versions, missing required fields,
  and unknown nested fields
- Maintain existing bare `Diagram` JSON wire representation and renderer output

M2 defers `PortId`, source provenance sidecar, style tokens, IDs for unnamed
relations / messages / transitions themselves, and frontend syntax support for
annotations. Semantic validation for duplicate annotation IDs, dangling targets,
and empty multi-element targets will be added in the milestone where frontends
generate annotations.

### M3x0: Exchange exporter contract bridge

Status: **Implemented**

- `LayoutOutput::export_input(&Diagram)` borrows diagram / scene / semantic layout
  and validates top-level variant, identity/order/index/semantic parity for 5 domains,
  and finite non-negative geometry
- `ExportInput` has no clone; exposes only private fields and getters
- Add strict `render_export` API for draw.io / Excalidraw / PowerPoint,
  maintaining identical bytes to legacy `render`
- Change only the 3 exchange formats in CLI to use strict contract; input boundaries
  for SVG / terminal / PNG / DOT remain unchanged
- Convert dangling graph/class/ER relations, dangling sequence participants, illegal
  state endpoints, and future enum fallbacks to layout errors
- Do not change IR schema or existing golden bytes

### M3: Existing diagram semantics

Status: **M3b5 (activation bar, schema V13) implemented**

Extend all 5 existing diagram types from their minimal per-frontend subsets
to an IR that preserves meaning.

1. Graph / Flowchart
   - **M3a1 implemented**: Down / Right / Up / Left
     - Native DSL `direction up|left`
     - Mermaid `BT` / `RL`
     - Main axis reversal for graph / class layout
     - DOT `rankdir=TB/LR/BT/RL`
     - Schema V3 and V1 / V2 document migration
   - **M3a2a-I implemented**: legacy Default / Rectangle / RoundedRectangle
     - Native DSL `shape rectangle|rounded`
     - Preserve shape for Mermaid bare / `[label]` / `(label)`
     - Propagate shape to layout and all backends
     - Schema V4 and V1 / V2 / V3 document migration
   - **M3a2a-II implemented**: Circle / Diamond
     - Native DSL `shape circle|diamond`
     - Mermaid `((label))` / `{label}` and last-wins update rule for explicit declarations
     - Shape-specific sizing, Scene path, edge endpoint clipping
     - Propagate shape to SVG / PNG / terminal / draw.io / DOT / Excalidraw / PPTX
     - Schema V5 and V1 / V2 / V3 / V4 document migration
   - **M3a2b implemented**: edge semantics / presentation
     - Schema V6. Legacy `Edge::new(..., ArrowType)` and legacy `arrow` wire bytes maintained
     - Add `from_arrow` (source marker), `line: Solid|Dashed|Dotted`,
       `weight: Normal|Thick` to `Edge`, and type directed / undirected / bidirectional
     - Native DSL: `a -> b` (directed) / `a --- b` (undirected) /
       `a <-> b` (bidirectional), plus `line solid|dashed|dotted` / `weight normal|thick`
       modifiers placed before `: "label"`, with canonical formatter output
     - Mermaid: add `-.->` / `-.-` / `==>` / `===` / `<-->` and `|label|` pipe-label subset
     - Source-end arrowhead layout retraction (retract route by the source-side arrow amount for bidirectional)
     - Propagate to all backends: Scene path, SVG / PNG / terminal stroke,
       DOT (`dir` / `style` / `penwidth`), draw.io (`startArrow` / `dashed` / `dashPattern` /
       `strokeWidth`), Excalidraw (`strokeStyle` / `strokeWidth` / `startArrowhead`),
       PPTX (`prstDash` / `w` / `headEnd`)
     - Extend M3x0 exchange exporter contract to include new edge fields in validation
     - Zero diff to existing goldens; only new `edge_presentation` golden added
   - **M3a3 implemented**: subgraph / container
     - Schema V7. Old documents upgrade losslessly to empty `containers: []`;
       non-empty `containers` explicitly rejected in V1-V6
     - Add tree structure `Container { id, label, members, children }` to `GraphDiagram`.
       `members` references ids in the flat `nodes` map (no duplication of node bodies
       in the container); root-level containers are in `containers`, nested containers
       in parent's `children`, both in declaration order
     - Native DSL: `subgraph id [: "label"] { <node decls + nested subgraph> }`.
       Body allows only node declarations and nested subgraphs; empty subgraphs,
       edge statements in body, and use in non-graph contexts (state / sequence) all rejected
     - Mermaid: `subgraph id [Title]` / bare title / nested subgraph +
       `end`. First mention of node determines membership.
       Per-subgraph `direction` not supported (Partial)
     - Layout uses naive bounding-box approach without changing node placement or edge routing
       (just draws a rectangle of the bounding box of the node group inside the container
       plus `CONTAINER_PAD`). Layout optimization considering containment deferred to M4
     - Add pre-order `containers: Vec<ContainerLayout>` to `SemanticLayout`
     - Propagate to all backends: SVG / PNG / terminal draw a dashed rectangle + top-left
       label string behind nodes; DOT uses native nested `subgraph cluster_N { label=...; }`;
       draw.io / Excalidraw / PPTX use backdrop style (no-fill dashed rectangle + independent
       label text) on the same coordinate system as nodes
     - Extend M3x0 exchange exporter contract to include container parity / geometry in validation
     - PlantUML excluded as there is no graph frontend / parser
       (existing PlantUML for 5 types covers sequence / state / class / ER only)
     - Zero diff to existing goldens; only new `subgraph` / `mermaid_subgraph` goldens added
   - **M3a4 implemented**: port
     - Schema V8. Old documents upgrade losslessly to edges without ports;
       non-None ports explicitly rejected in V2-V7 (V1 wire arm rejects first)
     - `Edge.from_port` / `to_port: Option<Port>` (4 cardinal directions only: North / East /
       South / West. `Port` is `#[non_exhaustive]`; ordinal / center / offset ports deferred
       to subsequent sub-milestones)
     - Native DSL: `a.north -> b.south`. `.` attaches directly to the endpoint
       (`a . north` is a syntax error); port words are non-reserved and valid as node IDs;
       unknown ports and use outside graph (state / sequence) are explicit errors;
       formatter outputs canonical full names (idempotent)
     - Mermaid: no port / compass syntax (Partial).
       Port equivalence covered by kozue-dsl unit tests
     - Layout snaps cardinal ports to side midpoints / vertices using existing shape clip
       (`clip_to_shape`) as axial unit ray reuse. Routing and node placement unchanged;
       edges without ports follow the old code path unchanged
     - DOT uses native compass (`"a":e -> "b":w`); draw.io uses
       `exitX/exitY/entryX/entryY` (fixed order: source exit → target entry);
       SVG / PNG / terminal / Excalidraw / PPTX use snapped routes with no code changes.
       Future `Port` variants explicitly rejected in all match arms
     - Add port parity and future variant rejection to M3x0 exchange exporter contract
     - Zero diff to existing goldens; only new `ports` golden added
2. Sequence
   - **M3b1 implemented**: participant kind
     - Schema V9. Old documents upgrade losslessly to no participant kind (Default);
       non-Default kinds explicitly rejected in V2-V8 (V1 wire arm rejects first)
     - Add `ParticipantKind` (Default / Actor / Boundary / Control / Entity /
       Database / Collections / Queue, `#[non_exhaustive]`) and introduce `Participant.kind`.
       Validated by `participant_kind_supported_in` gate and Sequence arm of `diagram_supported_in`
     - Native DSL: add `actor` / `boundary` / `control` / `entity` / `database` /
       `collections` / `queue <id>[: "label"]` as declaration keywords
       (`participant` is Default). Kind words are non-reserved and valid as IDs;
       formatter outputs canonical kind keywords (idempotent)
     - Mermaid: promote `actor X` to `ParticipantKind::Actor` (previously Partial).
       Other kinds have no Mermaid syntax and are excluded
     - PlantUML: promote actor / boundary / control /
       entity / database / collections / queue (previously collapsed to Participant)
       to kind-preserving (features table: Partial → Supported)
     - Layout / SemanticLayout: add `ParticipantLayout.kind`. Non-Default adds a uniform
       `«kind»` guillemet line above the label in the header; Default participant geometry
       is completely unchanged (elaborate rendering such as stick figures deferred to later polish).
       Header height increases only for non-Default
     - Propagate to all backends: SVG / PNG / terminal via Scene Text;
       draw.io / Excalidraw / PPTX reflect stereotype line in header shape label.
       Default output bytes unchanged. DOT maintains sequence non-support (`UnsupportedDiagram`).
       Future `ParticipantKind` variants explicitly rejected by exchange contract's
       `validate_export_semantics`; presentation path uses safe fallback
     - Add participant kind parity and future variant rejection to M3x0 exchange exporter contract
     - Zero diff to existing goldens; only new `seq_participant_kinds` (`.kzd` / `.svg` /
       `.drawio` / `.excalidraw` / `.pptx`) and `mermaid_seq_actor`
       (`.mmd` / `.svg`) goldens added
   - **M3b2 implemented**: message arrow / async (open / filled / cross / circle /
     async / bidirectional)
     - Schema V10. Old documents upgrade losslessly to the extent representable by arrow;
       Open / Cross / Circle head or non-None tail explicitly rejected in V2-V9
       (V1 wire arm rejects first)
     - **Shared `ArrowType` (used by graph / class / ER, Triangle / None) unchanged**;
       introduce sequence-specific `MessageArrow` (None / Filled / Open / Cross / Circle,
       `#[non_exhaustive]`). Replace `Message.arrow: ArrowType` with
       `head: MessageArrow` and add `tail: MessageArrow` (source end).
       Semantics: Filled=synchronous, Open=asynchronous, Cross=lost/destroy,
       Circle=found/circle-end, bidirectional = tail ≠ None.
       `Message::new(..., ArrowType)` maintained as compatible constructor
       (Triangle→Filled / None→None); add `Message::with_arrows`
     - Native DSL: add `head` / `tail` modifiers (`open|filled|cross|circle|none`)
       to messages, following the same convention as graph edge `line` / `weight` modifiers.
       `head`/`tail` are non-reserved and valid as IDs; formatter outputs
       canonical order (head→tail) and omits defaults (head filled / tail none)
       (idempotent). Misuse in graph / state and unknown values are explicit errors
     - Mermaid: add `-)` → Open / `-x` → Cross / `<<->>` → bidirectional.
       Correct `->` / `-->` to None head per actual Mermaid spec
       (resolve previous Partial treatment as Triangle. Existing goldens use only `->>` so zero diff)
     - PlantUML: promote `->>`→Open (previously Partial); enable `->x`→Cross / `->o`→Circle
       / `<->`→bidirectional (previously unsupported error). Add word-boundary guard for
       arrow token to prevent misidentification of `->oscar` etc.
     - Layout: `MessageLayout.head` / `tail`. Draw head-type glyph
       (Filled=filled triangle / Open=V-shape / Cross=× / Circle=octagonal approximation)
       at target and source ends, retracting route at both ends by glyph size. Default
       (Filled / None) coordinates and Scene generation match existing output at the expression
       level; existing straight / self-loop golden bytes unchanged. Circle uses a temporary
       octagonal approximation until M4 ellipse primitive (not clipped, extends beyond bounds)
     - Propagate to all backends: SVG / PNG / terminal via Scene; draw.io uses
       `startArrow` / `endArrow` (Filled→block/classic, Open→open, Circle→oval,
       Cross→cross); Excalidraw uses `startArrowhead` / `endArrowhead`
       (Filled→triangle, Open→arrow, Circle→dot, Cross→bar approximation); PPTX uses
       `headEnd` / `tailEnd` (Filled→triangle, Open→stealth, Circle→oval,
       Cross→diamond approximation). Cross in exchange formats is a lossy approximation
       but documented in doc-comment (not silent). DOT maintains sequence non-support
     - Add head / tail parity and `MessageArrow` future variant rejection to M3x0 contract
     - Zero diff to existing goldens; only new `seq_message_arrows` (`.kzd` / `.svg` / `.txt` /
       `.png` / `.drawio` / `.excalidraw` / `.pptx`), `mermaid_seq_arrows`
       (`.mmd` / `.svg`), and `plantuml_seq_arrows` (`.puml` / `.svg`) goldens added
   - **M3b3 implemented**: note + SemanticLayout item list generalization
     - Schema V11. Old documents upgrade losslessly to body without notes;
       Note items explicitly rejected in V2-V10 (`sequence_note_supported_in` gate,
       V1 wire arm rejects first). V11 added to all existing `*_supported_in` gates
     - Add `Note(Note)` to `SequenceItem`. `Note { text, position, targets:
       Vec<ElementId> }`. Position uses a dedicated **`NotePosition` (LeftOf / RightOf /
       Over, `#[non_exhaustive]`)** separate from annotation-system `NotePlacement`
       (which includes Auto/Above/Below that conflict with sequence semantics)
       (same principle as separating `MessageArrow` from `ArrowType`). Cardinality
       constraints: LeftOf/RightOf==1, Over>=1 (validated in frontend and layout)
     - **Pivot point**: redesign `SemanticLayout` sequence from "message-only 1:1" to
       **generalized item list**. Replace `SequenceLayout.messages: Vec<MessageLayout>`
       with `items: Vec<SequenceItemLayout>` (`Message(MessageLayout)` /
       `Note(NoteLayout)`). Change exchange contract's `items.len()==messages.len()`
       assumption to **item-parity** (zip by variant match between diagram.items[i] and
       layout.items[i]; cross-variant is explicit mismatch).
       Foundation for subsequent M3b7 (fragment recursive tree)
     - Native DSL: `note over a[, b...] : "text"` / `note left of a : "text"` /
       `note right of a : "text"`. `note`/`over`/`left`/`right`/`of` are non-reserved
       (fall-through to other alternatives on lookahead miss); items interleave with
       messages in declaration order; use in graph / state is explicit error;
       formatter outputs canonical form (idempotent)
     - Mermaid: promote `Note over/left of/right of` (`Note`/`note` both accepted) from
       unsupported; push to items in source line order
     - PlantUML: promote single-line `note over/left of/right of ... : text`.
       Multi-line `note ... end note` block / `hnote` / `rnote` are out of scope
       and remain explicit unsupported errors (no silent drop; features table updated)
     - Layout: note occupies 1 row; draw UML dog-ear (folded corner) Path outline + centered
       Text in Scene. Column width incorporates note width additively into existing
       label-width / self-overhang mechanism (when no notes, `col_x` expression matches
       current output exactly -> existing golden bytes unchanged). `NoteLayout { index, text,
       position, targets, rect, text_anchor }`. No fill, so lifeline shows through
       (temporary until M4 paint primitive)
     - Propagate to all backends: SVG / PNG / terminal via Scene with no code changes;
       draw.io uses `shape=note` vertex; Excalidraw uses rectangle approximation
       (UML note shape unavailable; loss documented in doc-comment); PPTX uses rect + text.
       DOT maintains sequence non-support
     - Add item-parity cross-check, note geometry validation, and `NotePosition` future
       variant rejection to contract
     - Zero diff to existing goldens; only new `seq_notes` (`.kzd` / `.svg` / `.txt` / `.png` /
       `.drawio` / `.excalidraw` / `.pptx`), `mermaid_seq_notes` (`.mmd` / `.svg`), and
       `plantuml_seq_notes` (`.puml` / `.svg`) goldens added
   - **M3b4 implemented**: divider / delay / reference
     - Schema V12. Old documents upgrade losslessly; divider / delay /
       reference items explicitly rejected in V2-V11 (new `sequence_divider_supported_in` /
       `sequence_delay_supported_in` / `sequence_reference_supported_in` gates, V12-only;
       V1 wire arm rejects first). V12 added to all 9 existing gates;
       `sequence_note_supported_in` extended to V11 | V12
     - Add `Divider(Divider)` / `Delay(Delay)` / `Reference(Reference)` to `SequenceItem`.
       `Divider { text }` / `Delay { text: Option<String> }` (typed to represent
       PlantUML `...` no-text gap) / `Reference { text, targets: Vec<ElementId> }`
       (targets: 1 or more in declaration order, same as Over note). All `#[non_exhaustive]`,
       `deny_unknown_fields`, `new()`. The Sequence arm of `diagram_supported_in` uses
       exhaustive match with no wildcard (fail-closed)
     - Native DSL: `divider : "text"` / `delay` (no text) / `delay : "text"` /
       `ref over a[, b...] : "text"`. `divider`/`delay`/`ref`/`over` are non-reserved.
       `divider` (`:` required) / `ref` (`over` required) fall through to node on lookahead
       miss; bare `delay` uses negative lookahead on `-`/`<` to fall through to edge
       (`delay -> b`). **Outside sequence, `divider`/`delay` are reinterpreted as
       ordinary node / state identifiers** (same convention as `participant`/`state`/`subgraph`
       being valid as IDs; delegated to shared helper at build time, fully reusing
       existing node/state collection logic). `ref over` maintains sequence-only explicit error.
       Formatter outputs canonical form (idempotent)
     - Mermaid: no divider / delay / reference syntax; no changes
       (no silent drops occur)
     - PlantUML: promote `== text ==` (divider) / `...` `...text...` (delay) / single-line
       `ref over a[, b...] : text` (reference) from unsupported. `||` spacer and
       multi-line `ref over ... end ref` block are out of scope and remain explicit
       unsupported (features table updated)
     - Layout: each item occupies 1 row. Divider is a full-width band (Rect outline + centered Text);
       delay is a full-width dotted line + optional centered label; reference is a span frame
       of targets + top-left "ref" tab + centered body text. Reference column width contribution
       reuses Over note mechanism (span_widths / overhang); divider / delay do not contribute
       to `col_x` expression -> existing golden bytes unchanged. `DividerLayout` / `DelayLayout`
       (`text_anchor: Option<Point>`) / `ReferenceLayout`
     - Propagate to all backends: SVG / PNG / terminal via Scene with no code changes;
       draw.io uses native `shape=umlFrame` for reference, rect for divider, dashed rect
       for delay (dotted→dashed approximation documented in doc-comment); Excalidraw uses
       rectangle + text approximation (tab / dotted styling loss documented in doc-comment);
       PPTX uses rect + text (reference has "ref\n" prefix). DOT maintains sequence non-support
     - Add item-parity cross-check (index + text + targets), geometry validation
       (delay has Option anchor), and future variant fail-closed to contract
     - Zero diff to existing goldens; only new `seq_dividers` (`.kzd` / `.svg` / `.txt` / `.png` /
       `.drawio` / `.excalidraw` / `.pptx`) and `plantuml_seq_dividers` (`.puml` / `.svg`) goldens added
   - **M3b5 implemented**: activation bar (activation interval — first interval model)
     - Schema V13. Old documents upgrade losslessly; Activate / Deactivate items explicitly
       rejected in V2-V12 (new `sequence_activation_supported_in` gate, V13-only).
       V13 added to all existing gates; deserialize per-version arms (V1-V13) and
       numeric boundary (accept 13 / reject 14) updated
     - Add `Activate(Activation)` / `Deactivate(Activation)` to `SequenceItem`
       (two variants because the two keywords appear separately in source; payload struct
       `Activation { participant }` shared). `#[non_exhaustive]`, `deny_unknown_fields`, `new()`
     - Layout: first interval model, unlike the flat leaf items through b4. Introduce
       `SequenceLayout.bars` (rect stack on lifelines) as a separate field from items.
       Pair `activate`↔`deactivate` using a per-participant stack; nesting steps right by
       depth at 3px intervals. Unpaired activate / excess deactivate are layout errors
       (no silent drop). Endpoints for messages / self-messages are adjusted to bar edges
       (`raw ± BAR_WIDTH/2 + (depth-1)*BAR_NEST_OFFSET`; nest term always added for
       consistency with bar geometry). In diagrams without activations, depth=0 so
       endpoints equal `col_x` — existing golden bytes unchanged. Each item carries a
       `SequenceItemLayout::Activation` marker for 1:1 parity; drawing rect is separated
       into `bars` (avoiding double draw). Bars drawn in depth-ascending order, behind
       messages (innermost bar is frontmost when nested)
     - Native DSL: `activate <id>` / `deactivate <id>`. `activate`/`deactivate` are non-reserved.
       Bare (no ID) falls through to node; `activate -> b` uses negative lookahead on `-`/`<`
       to fall through to edge; `activate : "X"` falls through to node.
       **Outside sequence, explicit error** (the 2-token form `activate <id>` was already
       invalid before M3b5 so no reinterpretation needed). Formatter is idempotent
     - PlantUML / Mermaid: promote `activate`/`deactivate` from unsupported. Undeclared
       participants auto-declare (same as message endpoints; PlantUML silent drop fixed;
       Mermaid already auto-declares). `++`/`--`/`+`/`-` message shorthands, `activate #color`,
       `return`/`create`/`destroy` remain explicitly unsupported (features table updated)
     - Layout: `ActivationBarLayout { rect, participant, depth }` /
       `ActivationMarkerLayout { index, participant, x, y, is_start, depth }`
     - Propagate to all backends: SVG / PNG / terminal via Scene with no code changes;
       draw.io / Excalidraw / PPTX map bars to rect. DOT maintains sequence non-support
     - Add item-parity cross-check (index + participant + is_start), bars geometry validation,
       and future variant fail-closed to contract
     - Zero diff to existing goldens; only new `seq_activation` (`.kzd` / `.svg` / `.txt` / `.png` /
       `.drawio` / `.excalidraw` / `.pptx`), `plantuml_seq_activation` (`.puml` / `.svg`), and
       `mermaid_seq_activation` (`.mmd` / `.svg`; byte-identical SVG with plantuml) goldens added
   - create / destroy
   - Recursive fragments: `loop` / `alt` / `opt` / `par` / `critical` / `break`
   - open / filled / cross / circle / async / bidirectional arrow
3. State
   - Hierarchical structure of composite states and regions
   - Typed pseudostates: Initial / Final / Choice / Fork / Join / History
   - State description and internal behavior
   - Transition trigger / guard / effect
4. Class
   - Structure members into visibility / name / type / parameter / modifier
   - Namespace / package containment
   - Preserve association / generalization / realization / dependency as semantic types
5. ER
   - Type keys and cardinality
   - Direction, constraint, and index metadata
   - Do not collapse attributes to formatted strings after layout

### M4: Layout and exchange exporter contract

Status: **Not started**

- Revisit the current lossy `SemanticLayout`
- Move layout output toward an `ElementId -> Geometry` mapping
- Pass both the original `Diagram` and geometry to exchange exporters
- Preserve shape, container, port, annotation, and structured members through to exporters
- Add paint, stroke, ellipse, image / icon, and semantic role to Scene primitives
- Prohibit silent skip of unknown primitives or unsupported semantic items

### M5: Shared new diagram families

Status: **Not started**

Add as domain-specific variants in order of commonality and utility.

1. Use case / Requirement
2. Component / Deployment / Architecture
3. Activity / Swimlane
4. Mindmap / Tree / WBS
5. Timeline / Gantt / Chronology
6. Network / structured data

### M6: Charts and specialized diagrams

Status: **Not started**

- Pie, XY, Radar, Quadrant, Sankey, Venn, etc. each have a dedicated semantic model
- Packet, Kanban, GitGraph, Timing, EBNF / Regex are independent variants as needed
- Salt, Ditaa, Math, etc. are lower priority; if opaque passthrough is adopted,
  only deterministic / hermetic inputs are allowed

## Source-specific extensions

Candidates that are not normalized into the shared IR:

- Mermaid frontmatter, theme variables, renderer directives, raw CSS, `classDef`
- PlantUML `skinparam`, `<style>`, raw Creole text, layout engine / pragma
- Delimiter, quote, and spelling for exact source round-trip
- Preprocessor macro / include definition information

PlantUML remote `!include`, `%load_json`, `%now`, and Gantt's `today` conflict
with the determinism requirement. The preprocessor is placed outside the IR, and
external inputs are prohibited by default. When permitted, a content hash and
fixed evaluation context are required.

## Contract tests

Add and maintain the following at each milestone.

- Equivalent inputs across frontends produce semantically identical IR
- IR schema fixture, round-trip, version migration / rejection
- Serialization of identical data is byte-identical
- Corresponding geometry exists for every semantic element
- Golden tests for the corresponding backend
- Unsupported features are not silently downgraded / silently dropped
- After running `UPDATE_GOLDEN=1 cargo test`, always verify the diff

## M1 / M2 Verification status

- `cargo fmt --check`: pass
- `cargo check --workspace`: pass
- `cargo test --workspace --no-run`: pass
- `cargo test --workspace --exclude kozue-cli`: pass
- 9 kozue-ir schema / migration / typed ID tests: pass
- CLI integration: 69 / 69 pass
- `cargo clippy --workspace --all-targets -- -D warnings`: pass
- `git diff --check`: pass

The class / ER draw.io goldens that remained in `drawio_class_er_goldens_match`
have been updated to match renderer output with correctly XML-escaped HTML inside the `value` attribute.
All 5 findings from the independent review have been addressed and re-reviewed;
no blocking findings remain.

## M3a1 Verification status

- `cargo test --workspace`: pass
- CLI integration: 70 / 70 pass
- 12 kozue-ir schema / migration tests: pass
- Tests for 4-direction graph / class, variable dimensions, dummy routes, bounds, and determinism: pass
- `cargo clippy --workspace --all-targets -- -D warnings`: pass
- `cargo fmt --check`: pass
- `git diff --check`: pass
- No blocking findings after independent review and two rounds of fix confirmation

## M3a2a-I Verification status

- Schema V4 migration and explicit shape rejection tests for old schemas: pass
- Native / Mermaid shape equivalence, formatter, and unsupported shape tests: pass
- Layout kind propagation, corner geometry, and route invariance tests: pass
- SVG / PNG / terminal / draw.io / DOT / Excalidraw / PPTX mapping tests: pass
- Only SVG golden updated due to Mermaid `[label]` being converted to Rectangle
- `cargo test --workspace` and workspace Clippy: pass

## M3a2a-II Verification status

- Schema V5 migration and V1-V4 node kind compatibility matrix tests: pass
- Native / Mermaid Circle / Diamond syntax, explicit declaration update rule, formatter tests: pass
- Sizing, fixed path order, and analytical endpoint clipping tests: pass
- All backend mapping integration tests and `node_shapes` goldens: pass

## M3a2b Verification status

- Schema V6 migration and V1-V5 document compatibility tests: pass
- Native DSL `->` / `---` / `<->` and `line` / `weight` modifier, formatter
  canonical output tests: pass
- Mermaid `-.->` / `-.-` / `==>` / `===` / `<-->` and pipe-label subset tests: pass
- `native_and_mermaid_edge_presentation_produce_equivalent_ir`: pass
  (Mermaid lacks a plain dashed graph edge token; dashed equivalence is covered
  separately by kozue-dsl / kozue-mermaid unit tests)
- Source-end arrowhead layout retraction tests: pass
- All backend mapping integration test (`edge_presentation_maps_across_all_backends`) and
  new `edge_presentation` golden (`.svg` / `.txt` / `.png` / `.dot` / `.drawio` /
  `.excalidraw` / `.pptx`): pass. Existing golden bytes unchanged
- Confirmed that the dashed-only / dotted-only / thick-only 3 PNG variants generate
  deterministically distinct bytes
- `strict_exchange_export_matches_legacy_bytes_for_all_domains_and_is_deterministic`
  including M3x0 exchange exporter contract extensions: pass
- `cargo fmt --check`: pass
- `cargo check --workspace`: pass
- `cargo test --workspace` (without `UPDATE_GOLDEN=1`): pass. CLI integration 75 / 75
  pass (case count equivalent to node_shapes + 1 new)
- `cargo clippy --workspace --all-targets -- -D warnings`: pass
- `git diff --check`: pass
- Independent review: no blocking / major findings. Of 2 minor findings: removed
  the span test that only verified trivially; changed SVG / PNG `Dotted` to an
  explicit arm with future-variant comment moved to its dedicated use. The issue
  where `line` / `weight` modifiers can absorb a same-named identifier on the next
  line is treated as a known property of the grammar, identical to the existing `shape` modifier
- New fields in `Edge` are required, same approach as when adding `Node.kind`
  (compatibility managed at schema envelope granularity, not per inner struct JSON bytes)

## M3a3 Verification status

- Schema V7 migration and explicit rejection tests for non-empty `containers` in V1-V6 documents: pass
- Native DSL `subgraph id [: "label"] { ... }`, nested subgraph,
  empty subgraph / edge in body / collision between subgraph ID and node ID / prohibition
  in state and sequence tests: pass
- Mermaid `subgraph` / `end`, bare title / `[Title]`, nested subgraph,
  first-mention membership tests: pass
- `native_and_mermaid_subgraphs_produce_equivalent_ir` /
  `native_and_mermaid_nested_subgraphs_produce_equivalent_ir`: pass
- Pre-order `SemanticLayout.containers`, bounding-box +
  `CONTAINER_PAD` geometry, invariance of existing node placement and edge routing tests: pass
- All backend mapping integration test (`subgraphs_map_across_all_backends`) and new `subgraph`
  golden (`.kzd` / `.svg` / `.txt` / `.png` / `.dot` / `.drawio` /
  `.excalidraw` / `.pptx`), new `mermaid_subgraph` golden (`.mmd` / `.svg`):
  pass. Existing golden bytes unchanged
- Visual inspection: confirmed SVG outputs dashed container rectangle behind (in rendering
  order) node rectangles, labeled containers have label string at top-left, and nested
  container (`inner`) fits inside parent container (`right`) rectangle.
  DOT: confirmed `cluster_2` is nested inside `subgraph cluster_0` / `cluster_1`,
  and only the labeled one has `label=`. draw.io: confirmed `dashed=1` backdrop cells
  `c0`/`c1`/`c2` appear before node cells `n0`-`n4`. Excalidraw: confirmed `dashed`
  rectangle elements and free-text elements only for labeled containers
  (`c0-label` / `c2-label`) appear before node elements. PPTX: confirmed no-fill
  rectangle shapes named `Container N` have `prstDash val="dash"` and appear
  before `Node N` shapes
- `strict_exchange_export_matches_legacy_bytes_for_all_domains_and_is_deterministic`
  including M3x0 exchange exporter contract extensions: pass
- `cargo fmt --check`: pass
- `cargo check --workspace`: pass
- `cargo test --workspace` (without `UPDATE_GOLDEN=1`): pass
- `cargo clippy --workspace --all-targets -- -D warnings`: pass
- `git diff --check`: pass
- When running `UPDATE_GOLDEN=1 cargo test`, `excalidraw_goldens_are_well_formed_json`
  / `pptx_goldens_are_well_formed_zip` transiently fail immediately after parallel test
  launch before new golden files are written (known parallel generation race). Pass on rerun
- Independent review: no blocking / major findings. 2 minor findings addressed:
  `direction` line inside subgraph only rejects as per-subgraph direction override
  when followed by a direction token (LR/RL/TB/BT/TD); a node named `direction`
  is interpreted identically inside and outside a subgraph (covered by test). The fact
  that IR does not re-validate empty containers on deserialize is documented in `Container`
  doc-comment (frontend guarantees non-empty; layout defensively generates degenerate box).
  As a nit, the DOT byte-compat test that only made trivial comparisons was reorganized
  to verify absence of clusters

## M3a4 Verification status

- Schema V8 migration (lossless upgrade from V1-V7) and explicit non-None port rejection
  tests for V2-V7 documents: pass. V1 excluded from port gate validation loop because
  fixture lacks `annotations` field and wire arm rejects first (noted in test comment)
- Native DSL port parse for all 4 edge operators, combined with modifiers / labels,
  unknown port error, `a . north` / `a. north` syntax error, port words as node IDs,
  port rejection in state / sequence, formatter canonical output (idempotent): pass
- Writing a port on `[*]` pseudo-state transition endpoints results in a parse error,
  no silent drop (`id_endpoint` remains without port support)
- Layout side midpoint / vertex snap (precise coordinate comparison for Rectangle / Circle /
  Diamond including non-square boxes), regression tests that port-less edge routes match
  old `clip_to_shape` path: pass. Confirmed arrow retraction needs no changes as it derives
  from route endpoints
- DOT compass suffix / port-less byte invariance, draw.io exit / entry fixed-order style /
  default byte invariance, contract port parity mismatch detection tests: pass
- All backend mapping integration test (`ports_map_across_all_backends`) and
  new `ports` golden (`.kzd` / `.svg` / `.txt` / `.png` / `.dot` / `.drawio` /
  `.excalidraw` / `.pptx`): pass. Existing golden bytes unchanged
  (confirmed `git diff --stat tests/golden` is empty)
- Port-specific strict exchange export test
  (`strict_exchange_export_matches_legacy_bytes_for_ports`): pass.
  Determinism tests for existing 5 domains unchanged by case list
- Visual inspection: confirmed in SVG that `a.east` endpoint snaps to right-edge midpoint
  of a rectangle at (93.70, 19.60); confirmed DOT golden contains both
  `"a":e -> "b":w` / `"b":s -> "c":n` and port-less `"a" -> "c"`
- First run of `UPDATE_GOLDEN=1 cargo test` reproduced the parallel generation race
  recorded in M3a3 (transient failure of excalidraw / pptx well-formed tests); all pass on rerun
- `cargo fmt --check` / `cargo check --workspace` / `cargo test --workspace`
  (without `UPDATE_GOLDEN`) / `cargo clippy --workspace --all-targets --
  -D warnings` / `git diff --check`: pass
- Independent review: no blocking / major / minor findings (verdict: ship).
  1 nit (V1 loop in port gate rejection test vacuously passes due to upstream rejection)
  addressed by removing V1 from the loop with an added comment

## M3b Design (Sub-milestone breakdown)

Sequence semantic extensions follow the same convention as M3a (schema+1 / all
frontends + layout + all backends + contract propagation / zero diff to existing
goldens / new goldens only / no silent drop / DOT maintains sequence non-support),
split into 7 sub-milestones ordered from orthogonal and flat (first) to structurally
invasive (last).

- **M3b1 participant kind (V9)**: implemented (above)
- **M3b2 message arrow / async (V10)**: implemented (above). Represent open / filled /
  cross / circle / async / bidirectional using `MessageArrow` head / tail
- M3b3 note + SemanticLayout item list generalization (V11): first non-Message item.
  This is the single point where the contract's `items.len()==messages.len()` 1:1 assumption
  and `SequenceLayout` are redesigned into a unified item list corresponding to diagram.items
  order (geometry unchanged, so existing golden bytes are unchanged). Deferring this would
  require incremental patches in b4-b7
- **M3b4 divider / delay / reference (V12)**: implemented (above). Reuses the new leaf
  item foundation from b3. Outside sequence, `divider`/`delay` are reinterpreted as
  ordinary node/state identifiers, preserving non-reserved behavior
- **M3b5 activation bar (V13)**: implemented (above). Introduce activation interval model
  (start / end, nesting). Add `SequenceLayout.bars` (rect stack on lifelines) separate
  from items; adjust message / self-message endpoints to bar edges. First interval model
  (through b4 was flat leaf items)
- M3b6 create / destroy (V14): participant lifecycle (header descends on create,
  × endpoint on destroy). Lifeline y range becomes variable
- M3b7 fragment loop / alt / opt / par / critical / break (V15): items change from
  flat Vec to recursive tree; contract zip becomes tree walk. Most invasive, placed last

## M3b1 Verification status

- Schema V9 migration (lossless upgrade from V1-V8) and explicit non-Default participant
  kind rejection tests for V2-V8 documents (`non_default_participant_kinds_require_schema_v9`):
  pass. V1 excluded from gate validation loop because fixture lacks `annotations` field
  and wire arm rejects first (same precedent as M3a4, noted in comment)
- Native DSL 8-kind keyword parse, kind word as ID (`kind_keyword_usable_as_id`),
  formatter idempotency (`fmt_participant_kinds_idempotent`) tests: pass
- Mermaid `actor` Actor promotion, PlantUML icon-variant keyword kind-preserving
  promotion tests: pass
- Equivalence: `native_and_plantuml_participant_kinds_produce_equivalent_ir` (all 7
  non-Default kinds) / `native_and_mermaid_actor_produce_equivalent_ir` (Actor only,
  Mermaid scope): pass
- Layout non-Default `«kind»` stereotype line, Default participant geometry invariance,
  mixed diagram (Default + Actor + Boundary) unified header height tests: pass
- All backend mapping (SVG / PNG / terminal via Scene Text; draw.io /
  Excalidraw / PPTX reflect header label; DOT maintains `UnsupportedDiagram`):
  pass. Existing all-Default golden bytes unchanged
- M3x0 exchange exporter contract participant kind parity and future variant
  rejection (`validate_export_semantics`): pass
- `cargo fmt --check` / `cargo check --workspace` / `cargo test --workspace`
  (without `UPDATE_GOLDEN`, all workspace green) / `cargo clippy --workspace
  --all-targets -- -D warnings` / `git diff --check`: pass
- Zero diff to existing goldens (`git diff --stat tests/golden` empty; new files are
  only `seq_participant_kinds` / `mermaid_seq_actor` untracked)
- Independent review (Opus): no blocking / major findings (verdict: ship).
  3 minor findings addressed: added DSL keyword-as-id / formatter idempotency tests,
  added intent comment to presentation fallback arm, appended participant kind to
  V8 wire error message. N2 (sharing stereotype helper for `ParticipantKind`) deferred
  as same pattern as NodeKind

## M3b2 Verification status

- Schema V10 migration (lossless upgrade from V1-V9) and explicit new arrow
  (Open / Cross / Circle head or non-None tail) rejection test for V2-V9 documents
  (`message_arrows_require_schema_v10`, validated across all 25 head/tail combinations
  x each legacy fixture, `is_ok()==legacy_ok`): pass. V1 excluded due to wire arm prior rejection
- **Shared `ArrowType` unchanged** (remains Triangle / None; no Cross etc. mixed in) confirmed.
  New markers isolated in `MessageArrow`
- Confirmed that default (head=Filled / tail=None) layout coordinates and all backend output
  are byte-identical to existing (straight / self-loop retraction / triangle vertex match
  at expression level; drawio/excalidraw/pptx default fragments also match legacy) ->
  existing seq goldens unchanged
- Native DSL `head` / `tail` modifier parse, non-reserved (usable as ID), formatter
  idempotency / canonical order / default omission, misuse in graph/state and explicit
  unknown value errors tests: pass
- Mermaid `-)` / `-x` / `<<->>` and `->` / `-->` → None correction, PlantUML `->>`→Open /
  `->x` / `->o` / `<->` tests: pass. Confirmed by grep that existing `*.mmd` goldens
  do not use `->` / `-->`, so no golden diff
- Equivalence: CLI tests for each arrow form native↔PlantUML / native↔Mermaid producing
  identical IR: pass
- All backend mapping and exhaustive explicit handling of all `MessageArrow` future
  variant match arms (layout / contract / drawio / excalidraw / pptx): pass. svg/png/term
  consume lowered Scene
- M3x0 contract head / tail parity and unknown variant rejection: pass
- `cargo fmt --check` / `cargo check --workspace` / `cargo test --workspace`
  (all workspace green, 731 passed) / `cargo clippy --workspace --all-targets --
  -D warnings` / `git diff --check`: pass
- Zero diff to existing goldens (`git diff --stat tests/golden` empty; new files are
  only `seq_message_arrows` / `mermaid_seq_arrows` / `plantuml_seq_arrows` untracked)
- Independent review (Opus): no blocking / major findings (verdict: ship).
  All 6 self-reported implementation points are non-issues to minor (drawio classic
  consistency / pptx diamond / excalidraw bar Cross lossy approximation documented in
  doc-comment, not silent / Circle overshoot included in bounds but not clipped).
  Of 2 nits: added intent comment to formatter future-variant fallback; PlantUML
  `->x` word-boundary left as tested trade-off

## M3b3 Verification status

- Schema V11 migration and Note item explicit rejection for V2-V10
  (`sequence_notes_require_schema_v11`, covering LeftOf / RightOf / Over exhaustively), V11
  round-trip, numeric boundary / upgrade tests updated for CURRENT=V11: pass
- Confirmed V11 is present in all 9 `*_supported_in` gates (none missing)
- Native DSL `note over / left of / right of`, interleave in declaration order, prohibition
  in graph / state, multiple target rejection for left/right, unknown participant, formatter
  idempotency tests: pass
- Mermaid `notes_parse_and_preserve_source_order` / `note_left_of_rejects_multiple_targets`,
  PlantUML `single_line_note_is_supported_and_ordered` /
  `multi_line_note_block_is_unsupported` / `hnote_is_unsupported`: pass
- `native_mermaid_plantuml_notes_produce_equivalent_ir` (all 3 frontends produce same note IR):
  pass
- Contract item-parity cross-check (length + variant match zip), note geometry validation,
  `NotePosition` future variant rejection, and existing sequence contract test updates: pass
- `notes_map_across_all_backends` (drawio `shape=note`×3, Excalidraw rectangle+text,
  pptx rect+text, SVG / terminal contain note text): pass
- New `seq_notes` (`.kzd` / `.svg` / `.txt` / `.png` / `.drawio` / `.excalidraw` /
  `.pptx`), `mermaid_seq_notes` (`.mmd` / `.svg`), `plantuml_seq_notes`
  (`.puml` / `.svg`) goldens added. Existing golden bytes unchanged
  (`git status` shows only new untracked files)
- `cargo fmt --all --check` / `cargo check --workspace` /
  `cargo clippy --workspace --all-targets -- -D warnings` /
  `cargo test --workspace` (without `UPDATE_GOLDEN`, 741 tests, 33 binaries green) /
  `git diff --check`: all pass
- Independent review (Opus): no blocking / major findings. Confirmed all IR gate counts
  at V11; confirmed item-parity contract fail-closed (variant cross-match is explicit mismatch);
  confirmed `col_x` expression-level invariance without notes (backed by existing golden
  byte match); confirmed note rect / text_anchor also translated under bounds normalization.
  Frontend equivalence test required by roadmap contract was missing and added during review.
  Known limitations: Excalidraw note uses rectangle approximation (documented in doc-comment);
  PlantUML block notes unsupported; note has no fill so lifeline shows through
  (deferred to M4 paint primitive)

## M3b4 Verification status

- Schema V12 migration (lossless upgrade from V1-V11) and explicit divider / delay /
  reference item rejection for V2-V11 (`sequence_{dividers,delays,references}_require_schema_v12`;
  delay covers None / Some, reference covers single / multiple targets): pass. V12 added to
  existing 9 gates; 3 new gates are V12-only; numeric boundary (reject 13) updated: pass
- Native DSL `divider : "t"` / `delay`[` : "t"`] / `ref over a[, b] : "t"` parse,
  formatter idempotency, unknown participant rejection: pass
- **Non-reserved keyword regressions detected and fixed during independent review from 2 angles**:
  (1) `delay -> b` using bare `delay` as message source became reserved (detected by Opus) ->
  fixed with negative lookahead on `-`/`<` to fall through to edge parser.
  (2) `divider : "…"` / bare and labeled `delay` (valid node/state declarations pre-M3b4)
  in graph / state contexts became reserved (detected by Fable comprehensive review) ->
  fixed by delegating to shared helper at build time (`collect_graph_node` / `collect_state_decl`),
  reinterpreting as same-named node/state. Aligned with the same non-reserved convention
  used by existing keywords like `participant`/`state`/`subgraph`.
  Covered by `divider_delay_reinterpreted_as_node_or_state_outside_sequence` /
  `divider_delay_reference_keywords_usable_as_id` for sequence /
  `reference_rejected_outside_sequence` tests
- PlantUML `== t ==` / `...` / `...t...` (`...t` accepted leniently) / single-line `ref over`
  promoted; `||` spacer and multi-line ref block remain explicitly unsupported; Mermaid has
  no syntax, no changes: pass. Native↔PlantUML equivalent IR test: pass
- Contract item-parity cross-check (index + text + targets; variant cross-match is explicit mismatch),
  geometry validation (delay has Option anchor), future variant rejection: pass
- All backend mapping (drawio native `shape=umlFrame` ×1, divider rect, delay dashed
  rect / Excalidraw rectangle + text / pptx rect + "ref" prefix, SVG / PNG / terminal via
  Scene, DOT `is_err()` rejection): pass
- `cargo fmt --all --check` / `cargo check --workspace` /
  `cargo clippy --workspace --all-targets -- -D warnings` /
  `cargo test --workspace` (without `UPDATE_GOLDEN`, 754 passed) / `git diff --check`:
  all pass
- Zero diff to existing goldens (`git diff --stat tests/golden` empty; new files are
  only `seq_dividers` (7 formats) / `plantuml_seq_dividers` (`.puml` / `.svg`) untracked)
- Independent review (Opus) + comprehensive review (Fable): verdict ship. Known limitations:
  reference column width contribution based on text without "ref" tab minimum width (68px);
  ref frame height slightly exceeds `MSG_ROW_HEIGHT`; delay label overlaps dotted line
  (all visual quality only, no golden breakage; adjustment possible in M3b5 or later or M4 paint);
  PlantUML `||` and multi-line ref/note blocks unsupported; divider / reference have no fill
  so lifeline shows through (deferred to M4)

## M3b5 Verification status

- Schema V13 migration (lossless upgrade from V1-V12) and explicit Activate / Deactivate item
  rejection for V2-V12 (`sequence_activations_require_schema_v13`): pass. V13 added to all
  existing gates; new `sequence_activation_supported_in` is V13-only; deserialize per-version
  arms (V1-V13) and numeric boundary (accept 13 / reject 14) updated: pass
- Native DSL `activate <id>` / `deactivate <id>` parse, formatter idempotency, unknown
  participant rejection, non-reserved behavior (explicit error outside sequence; bare/`: "X"`/`activate -> b`
  fall through to node/edge): pass
- Layout: pair activate↔deactivate with per-participant stack; nesting steps right by depth.
  Expression for adjusting message / self-message endpoints to bar edges is consistent with
  bar rect geometry with no off-by-one. Unpaired / excess deactivate are layout errors.
  Diagrams without activations have endpoints equal to `col_x`, existing golden bytes unchanged.
  Bars drawn in depth-ascending order, behind messages (innermost frontmost)
- PlantUML / Mermaid `activate`/`deactivate` promotion, auto-declare for undeclared participants
  (PlantUML silent drop fixed); shorthand / color / return remain explicitly unsupported.
  Native↔PlantUML↔Mermaid equivalent IR tests: pass
- Contract item-parity (index + participant + is_start), bars geometry, future variant rejection: pass
- All backend mapping (draw.io / Excalidraw / PPTX map bars to rect; SVG / PNG / terminal
  via Scene; DOT maintains sequence non-support): pass
- `cargo fmt --all --check` / `cargo check --workspace` /
  `cargo clippy --workspace --all-targets -- -D warnings` /
  `cargo test --workspace` (without `UPDATE_GOLDEN`, all workspace green) / `git diff --check`:
  all pass
- Zero diff to existing goldens (`git diff --stat tests/golden` empty; new files are
  only `seq_activation` (7 formats) / `plantuml_seq_activation` (`.puml` / `.svg`) /
  `mermaid_seq_activation` (`.mmd` / `.svg`) untracked)
- Independent review (Opus): 3 MAJOR findings detected and fixed — (1) sign bug in leftward
  nested (depth>=2) message endpoint (nest term changed to always add; regression test added),
  (2) nested bar z-order reversal (immediate push on deactivate -> splice after scan in
  depth-ascending order directly after lifeline, behind messages), (3) activate/deactivate
  silent drop for undeclared participants in PlantUML (invariant violation -> auto-declare;
  regression test added)
- Comprehensive review (Fable): verdict fix-then-ship. 1 blocker-class finding (Mermaid
  features table showing activate/deactivate as Unsupported was stale -> fixed to Supported).
  Known limitations: Mermaid `->>+`/`->>-`, PlantUML `activate #color`/`return` unsupported
  (explicit error, not silent); bar z-order is SVG=behind / exchange formats=front
  (visual difference only with 3+ participants); Excalidraw bar uses hachure

## Resume checklist

1. M3b1 / M3b2 / M3b3 / M3b4 / M3b5 are implemented, verified, independently reviewed, and
   comprehensively reviewed (M3b4 had 2 non-reserved keyword regressions; M3b5 had 3 MAJOR +
   1 stale comprehensive finding — all detected and fixed)
2. Next is **M3b6 create / destroy (V14)**. Participant lifecycle (header descends on create,
   × endpoint on destroy). Lifeline y range becomes variable. Second structurally invasive
   change following b5's activation interval model
3. Then M3b7 fragment loop/alt/opt/par/critical/break (V15, items go from flat Vec to recursive
   tree, contract zip becomes tree walk. Most invasive, placed last)
4. After implementation, perform an independent review by a separate reviewer and a root
   comprehensive review, confirming zero diff to existing goldens
5. Complete M3 in order: Sequence (M3b1-b7) -> State -> Class -> ER
