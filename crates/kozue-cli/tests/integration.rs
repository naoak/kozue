//! Integration tests for the kozue pipeline.
//!
//! - Golden tests: each `tests/golden/*.kzd` must render to the committed
//!   `*.svg` byte-for-byte.
//! - Mermaid golden tests: each `tests/golden/*.mmd` must render to the
//!   committed `*.svg` byte-for-byte.
//! - Determinism: rendering the same input twice gives identical output, tested
//!   by launching the CLI binary as a separate process (catches HashMap seed
//!   non-determinism across processes).
//! - DSL error case: an undeclared node reference fails to parse.

use std::path::PathBuf;

/// Workspace root (parent of the `crates/` directory).
fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR = .../crates/kozue-cli
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // crates
    p.pop(); // workspace root
    p
}

fn golden_dir() -> PathBuf {
    workspace_root().join("tests").join("golden")
}

fn compile(src: &str) -> String {
    let diagram = kozue_dsl::parse(src).expect("golden input must parse");
    let scene = kozue_layout::layout(&diagram).expect("golden layout must succeed");
    kozue_render_svg::render(&scene)
}

#[test]
fn native_and_mermaid_reverse_directions_produce_equivalent_ir() {
    let cases = [
        (
            "graph d { direction up\n a shape rectangle: \"A\"\n b shape rectangle: \"B\"\n a -> b }",
            "flowchart BT\n  a[A] --> b[B]\n",
        ),
        (
            "graph d { direction left\n a shape rectangle: \"A\"\n b shape rectangle: \"B\"\n a -> b }",
            "flowchart RL\n  a[A] --> b[B]\n",
        ),
    ];

    for (native, mermaid) in cases {
        assert_eq!(
            kozue_dsl::parse(native).expect("native parse"),
            kozue_mermaid::parse(mermaid).expect("Mermaid parse")
        );
    }
}

#[test]
fn native_and_mermaid_node_shapes_produce_equivalent_ir() {
    let native = "graph shapes {\n a shape rectangle: \"A\"\n b shape rounded: \"B\"\n c shape circle: \"C\"\n d shape diamond: \"D\"\n a -> b\n b -> c\n c -> d\n}";
    let mermaid = "flowchart TD\n  a[A] --> b(B) --> c((C)) --> d{D}\n";
    assert_eq!(
        kozue_dsl::parse(native).expect("native parse"),
        kozue_mermaid::parse(mermaid).expect("Mermaid parse")
    );
}

#[test]
fn native_and_mermaid_subgraphs_produce_equivalent_ir() {
    // `b` is given an explicit rectangle shape in native (`shape rectangle`)
    // to converge with Mermaid's `b[B]`, which always maps to
    // `NodeKind::Rectangle` — see `native_and_mermaid_node_shapes_produce_equivalent_ir`
    // above for the same convention.
    let native =
        "graph d {\n  a\n  subgraph x: \"X\" {\n    b shape rectangle: \"B\"\n  }\n  a -> b\n}";
    let mermaid = "flowchart TD\n  a\n  subgraph x [X]\n    b[B]\n  end\n  a --> b\n";
    assert_eq!(
        kozue_dsl::parse(native).expect("native parse"),
        kozue_mermaid::parse(mermaid).expect("Mermaid parse")
    );
}

#[test]
fn native_and_mermaid_nested_subgraphs_produce_equivalent_ir() {
    let native = "graph d {\n  subgraph outer: \"Outer\" {\n    a shape rectangle: \"A\"\n    subgraph inner {\n      b shape rectangle: \"B\"\n    }\n  }\n  a -> b\n}";
    let mermaid =
        "flowchart TD\n  subgraph outer [Outer]\n    a[A]\n    subgraph inner\n      b[B]\n    end\n  end\n  a --> b\n";
    assert_eq!(
        kozue_dsl::parse(native).expect("native parse"),
        kozue_mermaid::parse(mermaid).expect("Mermaid parse")
    );
}

#[test]
fn native_and_mermaid_edge_presentation_produce_equivalent_ir() {
    // Mermaid has no plain "dashed graph edge" token (dashed line style is
    // only reachable via sequence-diagram `-->>`), so dashed edges are
    // exercised separately in `kozue-dsl`/`kozue-mermaid`'s own test suites
    // rather than here — this test only covers the tokens both frontends
    // can express: dotted, thick, undirected, and bidirectional.
    let native = "graph d {\n a\n b\n c\n d\n e\n a -> b line dotted\n b --- c\n c <-> d\n d -> e weight thick\n}";
    let mermaid = "flowchart TD\n  a -.-> b\n  b --- c\n  c <--> d\n  d ==> e\n";
    assert_eq!(
        kozue_dsl::parse(native).expect("native parse"),
        kozue_mermaid::parse(mermaid).expect("Mermaid parse")
    );
}

#[test]
fn native_and_plantuml_participant_kinds_produce_equivalent_ir() {
    // Every non-Default participant kind must round-trip to the same semantic
    // IR from the native DSL and from PlantUML's icon-variant keywords.
    let native = "sequence s {\n  participant p: \"P\"\n  actor a: \"A\"\n  boundary b: \"B\"\n  control c: \"C\"\n  entity e: \"E\"\n  database d: \"D\"\n  collections co: \"Co\"\n  queue q: \"Q\"\n  a -> b : \"m\"\n}";
    let plantuml = "@startuml\nparticipant \"P\" as p\nactor \"A\" as a\nboundary \"B\" as b\ncontrol \"C\" as c\nentity \"E\" as e\ndatabase \"D\" as d\ncollections \"Co\" as co\nqueue \"Q\" as q\na -> b : m\n@enduml\n";
    assert_eq!(
        kozue_dsl::parse(native).expect("native parse"),
        kozue_plantuml::parse(plantuml).expect("PlantUML parse")
    );
}

#[test]
fn native_and_mermaid_actor_produce_equivalent_ir() {
    // Mermaid only expresses the Actor kind (via `actor`); other kinds have no
    // Mermaid syntax and are covered by the PlantUML equivalence test instead.
    let native = "sequence s {\n  actor a: \"a\"\n  participant b: \"b\"\n  a -> b : \"hello\"\n}";
    let mermaid = "sequenceDiagram\nactor a\nparticipant b\na->>b: hello\n";
    assert_eq!(
        kozue_dsl::parse(native).expect("native parse"),
        kozue_mermaid::parse(mermaid).expect("Mermaid parse")
    );
}

#[test]
fn native_mermaid_plantuml_notes_produce_equivalent_ir() {
    // Sequence notes must normalize to the same semantic IR across all three
    // frontends. Messages are omitted so the comparison isolates note
    // position/target/text/order (arrow-token mapping is covered separately).
    let native = "sequence s {\n  participant A\n  participant B\n  note over A : \"n1\"\n  note left of B : \"n2\"\n  note over A, B : \"n3\"\n}";
    let mermaid = "sequenceDiagram\n  participant A\n  participant B\n  Note over A: n1\n  Note left of B: n2\n  Note over A,B: n3\n";
    let plantuml =
        "@startuml\nparticipant A\nparticipant B\nnote over A: n1\nnote left of B: n2\nnote over A, B: n3\n@enduml\n";
    let native_ir = kozue_dsl::parse(native).expect("native parse");
    assert_eq!(
        native_ir,
        kozue_mermaid::parse(mermaid).expect("Mermaid parse")
    );
    assert_eq!(
        native_ir,
        kozue_plantuml::parse(plantuml).expect("PlantUML parse")
    );
}

#[test]
fn native_and_plantuml_dividers_delays_references_produce_equivalent_ir() {
    // Dividers, delays, and references must normalize to the same semantic IR
    // across the native and PlantUML frontends. (Mermaid has no syntax for any
    // of these three, so it is not part of the comparison.)
    let native = "sequence s {\n  participant A\n  participant B\n  divider : \"Phase 2\"\n  delay\n  delay : \"5 min\"\n  ref over A, B : \"auth\"\n}";
    let plantuml =
        "@startuml\nparticipant A\nparticipant B\n== Phase 2 ==\n...\n...5 min...\nref over A, B : auth\n@enduml\n";
    assert_eq!(
        kozue_dsl::parse(native).expect("native parse"),
        kozue_plantuml::parse(plantuml).expect("PlantUML parse")
    );
}

#[test]
fn native_and_plantuml_activation_produce_equivalent_ir() {
    // activate / deactivate must normalize to the same semantic IR
    // from the native DSL and from PlantUML.
    let native = r#"sequence s {
  participant A
  participant B
  A -> B : "request"
  activate B
  B --> A : "response"
  deactivate B
}"#;
    let plantuml = "@startuml
participant A
participant B
A -> B : request
activate B
B --> A : response
deactivate B
@enduml
";
    let mermaid = "sequenceDiagram
  participant A
  participant B
  A->>B: request
  activate B
  B-->>A: response
  deactivate B
";
    let native_ir = kozue_dsl::parse(native).expect("native parse");
    assert_eq!(
        native_ir,
        kozue_plantuml::parse(plantuml).expect("PlantUML parse")
    );
    assert_eq!(
        native_ir,
        kozue_mermaid::parse(mermaid).expect("Mermaid parse")
    );
}

#[test]
fn native_and_plantuml_message_arrows_produce_equivalent_ir() {
    // Every PlantUML message arrow form must produce the same semantic IR as
    // its native head/tail modifier spelling.
    let cases: &[(&str, &str)] = &[
        ("A -> B : \"m\"", "A -> B : m"),
        ("A --> B : \"m\"", "A --> B : m"),
        ("A -> B head open : \"m\"", "A ->> B : m"),
        ("A --> B head open : \"m\"", "A -->> B : m"),
        ("A -> B head cross : \"m\"", "A ->x B : m"),
        ("A --> B head cross : \"m\"", "A -->x B : m"),
        ("A -> B head circle : \"m\"", "A ->o B : m"),
        ("A --> B head circle : \"m\"", "A -->o B : m"),
        ("A -> B tail filled : \"m\"", "A <-> B : m"),
        ("A --> B tail filled : \"m\"", "A <--> B : m"),
    ];
    for (native_msg, plantuml_msg) in cases {
        let native = format!("sequence s {{\n  participant A\n  participant B\n  {native_msg}\n}}");
        let plantuml =
            format!("@startuml\nparticipant A\nparticipant B\n{plantuml_msg}\n@enduml\n");
        assert_eq!(
            kozue_dsl::parse(&native).expect("native parse"),
            kozue_plantuml::parse(&plantuml).expect("PlantUML parse"),
            "IR mismatch for `{plantuml_msg}`"
        );
    }
}

#[test]
fn native_and_mermaid_message_arrows_produce_equivalent_ir() {
    // Every Mermaid message arrow form expressible in the native DSL must
    // produce the same semantic IR. (Mermaid has no circle/found marker.)
    let cases: &[(&str, &str)] = &[
        ("A -> B : \"m\"", "A->>B: m"),
        ("A --> B : \"m\"", "A-->>B: m"),
        ("A -> B head open : \"m\"", "A-)B: m"),
        ("A --> B head open : \"m\"", "A--)B: m"),
        ("A -> B head cross : \"m\"", "A-xB: m"),
        ("A --> B head cross : \"m\"", "A--xB: m"),
        ("A -> B head none : \"m\"", "A->B: m"),
        ("A --> B head none : \"m\"", "A-->B: m"),
        ("A -> B tail filled : \"m\"", "A<<->>B: m"),
        ("A --> B tail filled : \"m\"", "A<<-->>B: m"),
    ];
    for (native_msg, mermaid_msg) in cases {
        let native = format!("sequence s {{\n  participant A\n  participant B\n  {native_msg}\n}}");
        let mermaid = format!("sequenceDiagram\nparticipant A\nparticipant B\n{mermaid_msg}\n");
        assert_eq!(
            kozue_dsl::parse(&native).expect("native parse"),
            kozue_mermaid::parse(&mermaid).expect("Mermaid parse"),
            "IR mismatch for `{mermaid_msg}`"
        );
    }
}

#[test]
fn explicit_node_shapes_map_across_all_backends() {
    let source = "graph shapes {\n d: \"Default\"\n r shape rectangle: \"Rectangle\"\n rr shape rounded: \"Rounded\"\n c shape circle: \"Circle\"\n dm shape diamond: \"Diamond\"\n}";
    let diagram = kozue_dsl::parse(source).unwrap();
    let output = kozue_layout::layout_full(&diagram).unwrap();

    let svg = kozue_render_svg::render(&output.scene);
    assert!(svg.contains("rx=\"4.00\""));
    assert!(svg.contains("rx=\"0.00\""));
    assert!(svg.contains("rx=\"8.00\""));
    assert_eq!(svg.matches("<polyline").count(), 2);

    let term = kozue_render_term::render(&output.scene);
    assert!(term.contains('┌'));
    assert!(term.contains('╭'));

    let drawio = kozue_render_drawio::render(&output.semantic).unwrap();
    assert!(drawio.contains("id=\"n0\" value=\"Default\" style=\"rounded=1;"));
    assert!(drawio.contains("id=\"n1\" value=\"Rectangle\" style=\"rounded=0;"));
    assert!(drawio.contains("id=\"n2\" value=\"Rounded\" style=\"rounded=1;"));
    assert!(drawio.contains("id=\"n3\" value=\"Circle\" style=\"ellipse;"));
    assert!(drawio.contains("id=\"n4\" value=\"Diamond\" style=\"rhombus;"));

    let dot = kozue_render_dot::render(&diagram).unwrap();
    assert!(dot.contains("\"d\" [label=\"Default\"]"));
    assert!(dot.contains("\"r\" [label=\"Rectangle\" shape=box style=\"\"]"));
    assert!(dot.contains("\"rr\" [label=\"Rounded\" shape=box style=rounded]"));
    assert!(dot.contains("\"c\" [label=\"Circle\" shape=circle style=\"\"]"));
    assert!(dot.contains("\"dm\" [label=\"Diamond\" shape=diamond style=\"\"]"));

    let excalidraw: serde_json::Value =
        serde_json::from_str(&kozue_render_excalidraw::render(&output.semantic).unwrap()).unwrap();
    let elements = excalidraw["elements"].as_array().unwrap();
    let roundness = |id: &str| {
        elements.iter().find(|element| element["id"] == id).unwrap()["roundness"].clone()
    };
    assert!(!roundness("n0").is_null());
    assert!(roundness("n1").is_null());
    assert!(!roundness("n2").is_null());
    let element_type = |id: &str| {
        elements.iter().find(|element| element["id"] == id).unwrap()["type"]
            .as_str()
            .unwrap()
    };
    assert_eq!(element_type("n3"), "ellipse");
    assert_eq!(element_type("n4"), "diamond");

    let pptx = kozue_render_pptx::render(&output.semantic).unwrap();
    let pptx_text = String::from_utf8_lossy(&pptx);
    assert!(pptx_text.contains("prst=\"rect\""));
    assert!(pptx_text.contains("prst=\"roundRect\""));
    assert!(pptx_text.contains("prst=\"ellipse\""));
    assert!(pptx_text.contains("prst=\"diamond\""));

    let png_for = |shape: &str| {
        let source = format!("graph one {{ n {shape}: \"Node\" }}");
        let diagram = kozue_dsl::parse(&source).unwrap();
        let scene = kozue_layout::layout(&diagram).unwrap();
        kozue_render_png::render(&scene).unwrap()
    };
    assert_ne!(png_for("shape rectangle"), png_for("shape rounded"));
    assert_ne!(png_for("shape circle"), png_for("shape diamond"));
    assert_ne!(png_for("shape rectangle"), png_for("shape circle"));
}

#[test]
fn edge_presentation_maps_across_all_backends() {
    let source = "graph edges {\n a: \"A\"\n b: \"B\"\n c: \"C\"\n d: \"D\"\n a -> b line dashed\n b -> c line dotted weight thick\n a --- c\n b <-> d weight thick\n}";
    let diagram = kozue_dsl::parse(source).unwrap();
    let output = kozue_layout::layout_full(&diagram).unwrap();

    let svg = kozue_render_svg::render(&output.scene);
    assert!(svg.contains("stroke-dasharray=\"6 4\""));
    assert!(svg.contains("stroke-dasharray=\"1.50 3.00\""));
    assert!(svg.contains("stroke-width=\"3.00\""));
    // Undirected edge: one polyline with no arrowhead polygon following it;
    // bidirectional edge: two arrowhead polygons framing its polyline.
    assert_eq!(svg.matches("<polyline").count(), 4);
    assert_eq!(svg.matches("<polygon").count(), 4);

    let dot = kozue_render_dot::render(&diagram).unwrap();
    assert!(dot.contains("style=dashed"));
    assert!(dot.contains("style=\"dotted,bold\""));
    assert!(dot.contains("penwidth=2"));
    assert!(dot.contains("dir=none"));
    assert!(dot.contains("dir=both"));
    assert!(dot.contains("style=bold"));

    let drawio = kozue_render_drawio::render(&output.semantic).unwrap();
    assert!(drawio.contains("dashed=1;"));
    assert!(drawio.contains("dashPattern="));
    assert!(drawio.contains("strokeWidth=3;"));
    assert!(drawio.contains("startArrow=classic"));
    assert!(drawio.contains("endArrow=none"));

    let excalidraw: serde_json::Value =
        serde_json::from_str(&kozue_render_excalidraw::render(&output.semantic).unwrap()).unwrap();
    let elements = excalidraw["elements"].as_array().unwrap();
    let edges: Vec<_> = elements
        .iter()
        .filter(|element| element["type"] == "arrow")
        .collect();
    assert!(edges.iter().any(|e| e["strokeStyle"] == "dashed"));
    assert!(edges.iter().any(|e| e["strokeStyle"] == "dotted"));
    assert!(edges.iter().any(|e| e["strokeWidth"] == 2.0));
    assert!(edges
        .iter()
        .any(|e| e["startArrowhead"].is_null() && e["endArrowhead"].is_null()));
    assert!(edges.iter().any(|e| !e["startArrowhead"].is_null()));

    let pptx = kozue_render_pptx::render(&output.semantic).unwrap();
    let pptx_text = String::from_utf8_lossy(&pptx);
    assert!(pptx_text.contains("prstDash val=\"dash\""));
    assert!(pptx_text.contains("prstDash val=\"sysDot\""));
    assert!(pptx_text.contains("w=\"38100\""));
    assert!(pptx_text.contains("headEnd"));

    let png_for = |edge: &str| {
        let source = format!("graph one {{ a: \"A\"\n b: \"B\"\n a {edge} }}");
        let diagram = kozue_dsl::parse(&source).unwrap();
        let scene = kozue_layout::layout(&diagram).unwrap();
        kozue_render_png::render(&scene).unwrap()
    };
    let dashed_only = png_for("-> b line dashed");
    let dotted_only = png_for("-> b line dotted");
    let thick_only = png_for("-> b weight thick");
    assert_ne!(dashed_only, dotted_only);
    assert_ne!(dashed_only, thick_only);
    assert_ne!(dotted_only, thick_only);
}

#[test]
fn notes_map_across_all_backends() {
    let source = "sequence notes {\n  participant a: \"Alice\"\n  participant b: \"Bob\"\n  note over a : \"thinking\"\n  a -> b : \"hello\"\n  note right of b : \"got it\"\n  note over a, b : \"both\"\n}";
    let diagram = kozue_dsl::parse(source).unwrap();
    let output = kozue_layout::layout_full(&diagram).unwrap();

    // SVG (presentation path): note text appears; there are >= 3 notes drawn.
    let svg = kozue_render_svg::render(&output.scene);
    assert!(svg.contains(">thinking<"));
    assert!(svg.contains(">got it<"));
    assert!(svg.contains(">both<"));

    // Terminal path renders the note text.
    let term = kozue_render_term::render(&output.scene);
    assert!(term.contains("thinking"));

    // draw.io: a `shape=note` vertex per note carrying the text.
    let drawio = kozue_render_drawio::render(&output.semantic).unwrap();
    assert!(drawio.contains("shape=note;"));
    assert!(drawio.contains("value=\"thinking\""));
    assert_eq!(drawio.matches("shape=note;").count(), 3);

    // Excalidraw: dog-eared `line` polygon (+ fold crease) + standalone text.
    let excalidraw: serde_json::Value =
        serde_json::from_str(&kozue_render_excalidraw::render(&output.semantic).unwrap()).unwrap();
    let elements = excalidraw["elements"].as_array().unwrap();
    assert!(elements
        .iter()
        .any(|e| e["type"] == "text" && e["text"] == "thinking"));
    // A note outline and its fold crease are both `line` elements.
    assert!(elements
        .iter()
        .any(|e| e["type"] == "line" && e["id"] == "note0"));
    assert!(elements
        .iter()
        .any(|e| e["type"] == "line" && e["id"] == "note0-fold"));

    // PPTX: `foldedCorner` (dog-eared) shape carrying the note text.
    let pptx = kozue_render_pptx::render(&output.semantic).unwrap();
    let pptx_text = String::from_utf8_lossy(&pptx);
    assert!(pptx_text.contains("thinking"));
    assert!(pptx_text.contains("Note "));
    assert!(pptx_text.contains("prst=\"foldedCorner\""));

    // DOT stays sequence-unsupported.
    assert!(kozue_render_dot::render(&diagram).is_err());
}

#[test]
fn dividers_delays_references_map_across_all_backends() {
    let source = "sequence dd {\n  participant a: \"Alice\"\n  participant b: \"Bob\"\n  divider : \"Phase 2\"\n  a -> b : \"hello\"\n  delay\n  delay : \"5 min\"\n  ref over a, b : \"auth\"\n}";
    let diagram = kozue_dsl::parse(source).unwrap();
    let output = kozue_layout::layout_full(&diagram).unwrap();

    // SVG (presentation path): divider/delay/reference text appears.
    let svg = kozue_render_svg::render(&output.scene);
    assert!(svg.contains(">Phase 2<"));
    assert!(svg.contains(">5 min<"));
    assert!(svg.contains(">auth<"));
    assert!(svg.contains(">ref<"));

    // Terminal path renders the text.
    let term = kozue_render_term::render(&output.scene);
    assert!(term.contains("Phase 2"));

    // draw.io: divider rect, dashed delay rect, and a native umlFrame reference.
    let drawio = kozue_render_drawio::render(&output.semantic).unwrap();
    assert!(drawio.contains("id=\"divider0\""));
    assert!(drawio.contains("value=\"Phase 2\""));
    assert!(drawio.contains("id=\"delay2\""));
    assert!(drawio.contains("id=\"delay3\""));
    assert!(drawio.contains("shape=umlFrame"));
    assert!(drawio.contains("id=\"ref4\""));

    // Excalidraw: rectangle `line` outlines + standalone text.
    let excalidraw: serde_json::Value =
        serde_json::from_str(&kozue_render_excalidraw::render(&output.semantic).unwrap()).unwrap();
    let elements = excalidraw["elements"].as_array().unwrap();
    assert!(elements
        .iter()
        .any(|e| e["type"] == "text" && e["text"] == "Phase 2"));
    assert!(elements
        .iter()
        .any(|e| e["type"] == "text" && e["text"] == "auth"));
    assert!(elements
        .iter()
        .any(|e| e["type"] == "line" && e["id"] == "ref4"));

    // PPTX: rect shapes carrying divider/delay text and a "ref"-prefixed frame.
    let pptx = kozue_render_pptx::render(&output.semantic).unwrap();
    let pptx_text = String::from_utf8_lossy(&pptx);
    assert!(pptx_text.contains("Phase 2"));
    assert!(pptx_text.contains("5 min"));
    assert!(pptx_text.contains("ref"));
    assert!(pptx_text.contains("Divider "));
    assert!(pptx_text.contains("Reference "));

    // DOT stays sequence-unsupported.
    assert!(kozue_render_dot::render(&diagram).is_err());
}

#[test]
fn subgraphs_map_across_all_backends() {
    let source = "graph subgraphs {\n  entry: \"Entry\"\n  subgraph left: \"Left Side\" {\n    a: \"A\"\n    b: \"B\"\n  }\n  subgraph right {\n    c: \"C\"\n    subgraph inner: \"Inner\" {\n      d: \"D\"\n    }\n  }\n  entry -> a\n  a -> b\n  b -> c\n  c -> d\n}";
    let diagram = kozue_dsl::parse(source).unwrap();
    let output = kozue_layout::layout_full(&diagram).unwrap();

    let svg = kozue_render_svg::render(&output.scene);
    assert!(svg.contains("stroke-dasharray=\"6 4\""));
    assert!(svg.contains(">Left Side<"));
    assert!(svg.contains(">Inner<"));

    let term = kozue_render_term::render(&output.scene);
    assert!(term.contains("Left Side"));
    assert!(term.contains("Inner"));

    let dot = kozue_render_dot::render(&diagram).unwrap();
    assert!(dot.contains("subgraph cluster_"));
    assert!(dot.contains("label=\"Left Side\""));
    assert!(dot.contains("label=\"Inner\""));

    let drawio = kozue_render_drawio::render(&output.semantic).unwrap();
    assert!(drawio.contains("dashed=1;"));
    assert!(drawio.contains("value=\"Left Side\""));

    let excalidraw: serde_json::Value =
        serde_json::from_str(&kozue_render_excalidraw::render(&output.semantic).unwrap()).unwrap();
    let elements = excalidraw["elements"].as_array().unwrap();
    assert!(elements
        .iter()
        .any(|e| e["type"] == "rectangle" && e["strokeStyle"] == "dashed"));
    assert!(elements
        .iter()
        .any(|e| e["type"] == "text" && e["text"] == "Left Side"));

    let pptx = kozue_render_pptx::render(&output.semantic).unwrap();
    let pptx_text = String::from_utf8_lossy(&pptx);
    assert!(pptx_text.contains("prstDash val=\"dash\""));
    assert!(pptx_text.contains("Container"));

    let png_with_subgraphs = kozue_render_png::render(&output.scene).unwrap();
    let flat_source = "graph subgraphs {\n  entry: \"Entry\"\n  a: \"A\"\n  b: \"B\"\n  c: \"C\"\n  d: \"D\"\n  entry -> a\n  a -> b\n  b -> c\n  c -> d\n}";
    let flat_diagram = kozue_dsl::parse(flat_source).unwrap();
    let flat_scene = kozue_layout::layout(&flat_diagram).unwrap();
    let png_without_subgraphs = kozue_render_png::render(&flat_scene).unwrap();
    assert_ne!(png_with_subgraphs, png_without_subgraphs);
}

#[test]
fn ports_map_across_all_backends() {
    let source = "graph ports {\n  a: \"A\"\n  b shape circle: \"B\"\n  c shape diamond: \"C\"\n  a.east -> b.west : \"x\"\n  b.south -> c.north\n  a -> c line dashed\n}";
    let diagram = kozue_dsl::parse(source).unwrap();
    let output = kozue_layout::layout_full(&diagram).unwrap();

    let dot = kozue_render_dot::render(&diagram).unwrap();
    assert!(dot.contains("\"a\":e ->"));
    assert!(dot.contains(":w"));
    assert!(dot.contains("\"b\":s ->"));
    assert!(dot.contains(":n"));

    let drawio = kozue_render_drawio::render(&output.semantic).unwrap();
    assert!(drawio.contains("exitX=1"));
    assert!(drawio.contains("entryX=0"));
    assert!(drawio.contains("exitX=0.5;exitY=1"));
    assert!(drawio.contains("entryX=0.5;entryY=0"));

    let flat_source = "graph ports {\n  a: \"A\"\n  b shape circle: \"B\"\n  c shape diamond: \"C\"\n  a -> b : \"x\"\n  b -> c\n  a -> c line dashed\n}";
    let flat_diagram = kozue_dsl::parse(flat_source).unwrap();
    let flat_output = kozue_layout::layout_full(&flat_diagram).unwrap();

    let svg = kozue_render_svg::render(&output.scene);
    let flat_svg = kozue_render_svg::render(&flat_output.scene);
    assert_ne!(svg, flat_svg);

    let excalidraw = kozue_render_excalidraw::render(&output.semantic).unwrap();
    let flat_excalidraw = kozue_render_excalidraw::render(&flat_output.semantic).unwrap();
    assert_ne!(excalidraw, flat_excalidraw);

    let pptx = kozue_render_pptx::render(&output.semantic).unwrap();
    let flat_pptx = kozue_render_pptx::render(&flat_output.semantic).unwrap();
    assert_ne!(pptx, flat_pptx);

    let png = kozue_render_png::render(&output.scene).unwrap();
    let flat_png = kozue_render_png::render(&flat_output.scene).unwrap();
    assert_ne!(png, flat_png);

    let term = kozue_render_term::render(&output.scene);
    let flat_term = kozue_render_term::render(&flat_output.scene);
    assert_ne!(term, flat_term);
}

#[test]
fn strict_exchange_export_matches_legacy_bytes_for_all_domains_and_is_deterministic() {
    for name in [
        "chain",
        "seq_basic",
        "state_basic",
        "class_basic",
        "er_basic",
    ] {
        let source = std::fs::read_to_string(golden_dir().join(format!("{name}.kzd"))).unwrap();
        let diagram = kozue_dsl::parse(&source).unwrap();
        let output = kozue_layout::layout_full(&diagram).unwrap();
        let input = output.export_input(&diagram).unwrap();

        let drawio = kozue_render_drawio::render_export(&input).unwrap();
        assert_eq!(
            drawio,
            kozue_render_drawio::render(&output.semantic).unwrap()
        );
        assert_eq!(drawio, kozue_render_drawio::render_export(&input).unwrap());

        let excalidraw = kozue_render_excalidraw::render_export(&input).unwrap();
        assert_eq!(
            excalidraw,
            kozue_render_excalidraw::render(&output.semantic).unwrap()
        );
        assert_eq!(
            excalidraw,
            kozue_render_excalidraw::render_export(&input).unwrap()
        );

        let pptx = kozue_render_pptx::render_export(&input).unwrap();
        assert_eq!(pptx, kozue_render_pptx::render(&output.semantic).unwrap());
        assert_eq!(pptx, kozue_render_pptx::render_export(&input).unwrap());
    }
}

#[test]
fn strict_exchange_export_matches_legacy_bytes_for_ports() {
    // Dedicated coverage for the strict export path with cardinal ports:
    // the existing 5-case loop above stays byte-invariant, and this
    // asserts render_export == render (including exit/entry and compass
    // suffixes carried through the strict contract) for the ports input.
    let source = std::fs::read_to_string(golden_dir().join("ports.kzd")).unwrap();
    let diagram = kozue_dsl::parse(&source).unwrap();
    let output = kozue_layout::layout_full(&diagram).unwrap();
    let input = output.export_input(&diagram).unwrap();

    let drawio = kozue_render_drawio::render_export(&input).unwrap();
    assert_eq!(
        drawio,
        kozue_render_drawio::render(&output.semantic).unwrap()
    );
    assert_eq!(drawio, kozue_render_drawio::render_export(&input).unwrap());

    let excalidraw = kozue_render_excalidraw::render_export(&input).unwrap();
    assert_eq!(
        excalidraw,
        kozue_render_excalidraw::render(&output.semantic).unwrap()
    );
    assert_eq!(
        excalidraw,
        kozue_render_excalidraw::render_export(&input).unwrap()
    );

    let pptx = kozue_render_pptx::render_export(&input).unwrap();
    assert_eq!(pptx, kozue_render_pptx::render(&output.semantic).unwrap());
    assert_eq!(pptx, kozue_render_pptx::render_export(&input).unwrap());
}

const GOLDEN_CASES: &[&str] = &[
    "chain",
    "branch",
    "right",
    "cycle",
    "skip",
    "wide_right",
    "node_shapes",
    "edge_presentation",
    "subgraph",
    "ports",
];

const SEQ_GOLDEN_CASES: &[&str] = &[
    "seq_basic",
    "seq_self_dashed",
    "seq_minimal",
    "seq_participant_kinds",
    "seq_message_arrows",
    "seq_notes",
    "seq_dividers",
    "seq_activation",
];

#[test]
fn golden_svgs_match() {
    for name in GOLDEN_CASES {
        let kzd = golden_dir().join(format!("{name}.kzd"));
        let svg_path = golden_dir().join(format!("{name}.svg"));
        let src =
            std::fs::read_to_string(&kzd).unwrap_or_else(|e| panic!("read {}: {e}", kzd.display()));
        let actual = compile(&src);

        // Allow regenerating goldens with UPDATE_GOLDEN=1.
        if std::env::var("UPDATE_GOLDEN").is_ok() {
            std::fs::write(&svg_path, &actual).unwrap();
            continue;
        }

        let expected = std::fs::read_to_string(&svg_path).unwrap_or_else(|e| {
            panic!(
                "read golden {}: {e} (run with UPDATE_GOLDEN=1 to create it)",
                svg_path.display()
            )
        });
        assert_eq!(
            actual, expected,
            "golden mismatch for {name}.kzd (run with UPDATE_GOLDEN=1 to update)"
        );
    }
}

/// Verify that rendering is deterministic across separate process invocations.
///
/// This catches non-determinism caused by HashMap random seeds or other
/// process-level sources of randomness, which an in-process check cannot detect.
#[test]
fn rendering_is_deterministic_across_processes() {
    let kzd = golden_dir().join("branch.kzd");
    let bin = env!("CARGO_BIN_EXE_kozue");

    // Run the CLI twice, writing to temporary files.
    let tmp = std::env::temp_dir();
    let out1 = tmp.join("kozue_det_test_1.svg");
    let out2 = tmp.join("kozue_det_test_2.svg");

    let status1 = std::process::Command::new(bin)
        .args([
            "render",
            kzd.to_str().unwrap(),
            "-o",
            out1.to_str().unwrap(),
        ])
        .status()
        .expect("failed to run kozue (first run)");
    assert!(status1.success(), "first kozue run failed");

    let status2 = std::process::Command::new(bin)
        .args([
            "render",
            kzd.to_str().unwrap(),
            "-o",
            out2.to_str().unwrap(),
        ])
        .status()
        .expect("failed to run kozue (second run)");
    assert!(status2.success(), "second kozue run failed");

    let svg1 = std::fs::read(&out1).expect("read first output");
    let svg2 = std::fs::read(&out2).expect("read second output");
    let _ = std::fs::remove_file(&out1);
    let _ = std::fs::remove_file(&out2);

    assert_eq!(
        svg1, svg2,
        "same input must produce byte-identical SVG across separate process invocations"
    );
}

/// Numeric validity of every golden layout: node boxes stay inside the
/// normalized scene bounds and never overlap each other.
#[test]
fn golden_layouts_are_well_formed() {
    use kozue_ir::{Scene, SceneItem};

    fn node_rects(scene: &Scene) -> Vec<&kozue_ir::Rect> {
        scene
            .items
            .iter()
            .filter_map(|i| match i {
                SceneItem::Rect(r) => Some(r),
                _ => None,
            })
            .collect()
    }

    for name in GOLDEN_CASES {
        let kzd = golden_dir().join(format!("{name}.kzd"));
        let src = std::fs::read_to_string(&kzd).unwrap();
        let diagram = kozue_dsl::parse(&src).expect("golden input must parse");
        let scene = kozue_layout::layout(&diagram).expect("golden layout must succeed");
        let rects = node_rects(&scene);

        // Every node box lies inside the scene bounds.
        for r in &rects {
            assert!(
                r.x >= -1e-6
                    && r.y >= -1e-6
                    && r.x + r.width <= scene.width + 1e-6
                    && r.y + r.height <= scene.height + 1e-6,
                "{name}: node box out of scene bounds"
            );
        }

        // No two node boxes overlap.
        for i in 0..rects.len() {
            for j in (i + 1)..rects.len() {
                let (a, b) = (rects[i], rects[j]);
                let disjoint = a.x + a.width <= b.x + 1e-6
                    || b.x + b.width <= a.x + 1e-6
                    || a.y + a.height <= b.y + 1e-6
                    || b.y + b.height <= a.y + 1e-6;
                assert!(disjoint, "{name}: node boxes {i} and {j} overlap");
            }
        }
    }
}

/// Straight chains stay collinear after the Sugiyama pipeline.
#[test]
fn golden_chains_are_collinear() {
    use kozue_ir::SceneItem;

    // chain.kzd: direction down → all node centers share one X.
    let src = std::fs::read_to_string(golden_dir().join("chain.kzd")).unwrap();
    let diagram = kozue_dsl::parse(&src).unwrap();
    let scene = kozue_layout::layout(&diagram).unwrap();
    let centers_x: Vec<f64> = scene
        .items
        .iter()
        .filter_map(|i| match i {
            SceneItem::Rect(r) => Some(r.x + r.width / 2.0),
            _ => None,
        })
        .collect();
    assert!(centers_x.len() >= 2);
    for cx in &centers_x[1..] {
        assert!(
            (cx - centers_x[0]).abs() < 1e-6,
            "chain.kzd nodes must be vertically aligned"
        );
    }

    // right.kzd: direction right → all node centers share one Y.
    let src = std::fs::read_to_string(golden_dir().join("right.kzd")).unwrap();
    let diagram = kozue_dsl::parse(&src).unwrap();
    let scene = kozue_layout::layout(&diagram).unwrap();
    let centers_y: Vec<f64> = scene
        .items
        .iter()
        .filter_map(|i| match i {
            SceneItem::Rect(r) => Some(r.y + r.height / 2.0),
            _ => None,
        })
        .collect();
    assert!(centers_y.len() >= 2);
    for cy in &centers_y[1..] {
        assert!(
            (cy - centers_y[0]).abs() < 1e-6,
            "right.kzd nodes must be horizontally aligned"
        );
    }
}

#[test]
fn undeclared_node_is_error() {
    let src = "graph d {\n a: \"A\"\n a -> ghost\n}";
    let result = kozue_dsl::parse(src);
    assert!(result.is_err(), "undeclared node must be an error");
    let errs = result.unwrap_err();
    assert!(
        errs.iter().any(|e| e.message.contains("unknown node")),
        "error should mention unknown node, got: {errs:?}"
    );
}

#[test]
fn seq_golden_svgs_match() {
    for name in SEQ_GOLDEN_CASES {
        let kzd = golden_dir().join(format!("{name}.kzd"));
        let svg_path = golden_dir().join(format!("{name}.svg"));
        let src =
            std::fs::read_to_string(&kzd).unwrap_or_else(|e| panic!("read {}: {e}", kzd.display()));
        let actual = compile(&src);

        // Allow regenerating goldens with UPDATE_GOLDEN=1.
        if std::env::var("UPDATE_GOLDEN").is_ok() {
            std::fs::write(&svg_path, &actual).unwrap();
            continue;
        }

        let expected = std::fs::read_to_string(&svg_path).unwrap_or_else(|e| {
            panic!(
                "read golden {}: {e} (run with UPDATE_GOLDEN=1 to create it)",
                svg_path.display()
            )
        });
        assert_eq!(
            actual, expected,
            "golden mismatch for {name}.kzd (run with UPDATE_GOLDEN=1 to update)"
        );
    }
}

#[test]
fn seq_rendering_is_deterministic_across_processes() {
    let kzd = golden_dir().join("seq_minimal.kzd");
    let bin = env!("CARGO_BIN_EXE_kozue");

    let tmp = std::env::temp_dir();
    let out1 = tmp.join("kozue_seq_det_test_1.svg");
    let out2 = tmp.join("kozue_seq_det_test_2.svg");

    let status1 = std::process::Command::new(bin)
        .args([
            "render",
            kzd.to_str().unwrap(),
            "-o",
            out1.to_str().unwrap(),
        ])
        .status()
        .expect("failed to run kozue (first run)");
    assert!(status1.success(), "first kozue run failed");

    let status2 = std::process::Command::new(bin)
        .args([
            "render",
            kzd.to_str().unwrap(),
            "-o",
            out2.to_str().unwrap(),
        ])
        .status()
        .expect("failed to run kozue (second run)");
    assert!(status2.success(), "second kozue run failed");

    let svg1 = std::fs::read(&out1).expect("read first output");
    let svg2 = std::fs::read(&out2).expect("read second output");
    let _ = std::fs::remove_file(&out1);
    let _ = std::fs::remove_file(&out2);

    assert_eq!(
        svg1, svg2,
        "same input must produce byte-identical SVG across separate process invocations"
    );
}

#[test]
fn unknown_participant_is_error() {
    let src = "sequence seq {\n  participant a: \"A\"\n  a -> ghost : \"msg\"\n}";
    let result = kozue_dsl::parse(src);
    assert!(result.is_err());
    let errs = result.unwrap_err();
    assert!(
        errs.iter()
            .any(|e| e.message.contains("unknown participant")),
        "error should mention unknown participant, got: {errs:?}"
    );
}

#[test]
fn duplicate_participant_is_error() {
    let src = "sequence seq {\n  participant a: \"A\"\n  participant a: \"B\"\n}";
    let result = kozue_dsl::parse(src);
    assert!(result.is_err());
    let errs = result.unwrap_err();
    assert!(
        errs.iter()
            .any(|e| e.message.contains("duplicate participant")),
        "error should mention duplicate participant, got: {errs:?}"
    );
}

#[test]
fn mixing_participant_and_node_is_error() {
    // With keyword-based dispatch (no signal inference), a plain node
    // declaration inside a `sequence` diagram is rejected explicitly.
    let src = "sequence seq {\n  participant a: \"A\"\n  b: \"B\"\n}";
    let result = kozue_dsl::parse(src);
    assert!(result.is_err());
    let errs = result.unwrap_err();
    assert!(
        errs.iter()
            .any(|e| e.message.contains("not valid in sequence diagrams")),
        "error should reject the plain node declaration, got: {errs:?}"
    );
}

#[test]
fn dashed_edge_in_graph_is_error() {
    let src = "graph d {\n  a: \"A\"\n  b: \"B\"\n  a --> b\n}";
    let result = kozue_dsl::parse(src);
    assert!(
        result.is_err(),
        "dashed edge in graph diagram must be an error"
    );
}

#[test]
fn seq_long_label_widens_columns() {
    let src = r#"sequence seq {
  participant a: "A"
  participant b: "B"
  a -> b : "this is a very long message label that should widen the columns"
}"#;
    let diagram = kozue_dsl::parse(src).expect("should parse");
    let scene = kozue_layout::layout(&diagram).expect("should layout");

    let src_short = r#"sequence seq {
  participant a: "A"
  participant b: "B"
  a -> b : "hi"
}"#;
    let diagram_short = kozue_dsl::parse(src_short).expect("should parse");
    let scene_short = kozue_layout::layout(&diagram_short).expect("should layout");
    assert!(
        scene.width > scene_short.width,
        "long label scene ({}) should be wider than short label scene ({})",
        scene.width,
        scene_short.width
    );
}

// ---------------------------------------------------------------------------
// Mermaid golden tests
// ---------------------------------------------------------------------------

const MERMAID_GOLDEN_CASES: &[&str] = &[
    "mermaid_flow",
    "mermaid_seq",
    "mermaid_state",
    "mermaid_class",
    "mermaid_er",
    "mermaid_subgraph",
    "mermaid_seq_actor",
    "mermaid_seq_arrows",
    "mermaid_seq_notes",
    "mermaid_seq_activation",
];

fn compile_mermaid(src: &str) -> String {
    let diagram = kozue_mermaid::parse(src).expect("mermaid golden input must parse");
    let scene = kozue_layout::layout(&diagram).expect("mermaid golden layout must succeed");
    kozue_render_svg::render(&scene)
}

#[test]
fn mermaid_golden_svgs_match() {
    for name in MERMAID_GOLDEN_CASES {
        let mmd = golden_dir().join(format!("{name}.mmd"));
        let svg_path = golden_dir().join(format!("{name}.svg"));
        let src =
            std::fs::read_to_string(&mmd).unwrap_or_else(|e| panic!("read {}: {e}", mmd.display()));
        let actual = compile_mermaid(&src);

        if std::env::var("UPDATE_GOLDEN").is_ok() {
            std::fs::write(&svg_path, &actual).unwrap();
            continue;
        }

        let expected = std::fs::read_to_string(&svg_path).unwrap_or_else(|e| {
            panic!(
                "read golden {}: {e} (run with UPDATE_GOLDEN=1 to create it)",
                svg_path.display()
            )
        });
        assert_eq!(
            actual, expected,
            "mermaid golden mismatch for {name}.mmd (run with UPDATE_GOLDEN=1 to update)"
        );
    }
}

/// Verify that mermaid rendering is deterministic across separate process invocations.
#[test]
fn mermaid_rendering_is_deterministic_across_processes() {
    let mmd = golden_dir().join("mermaid_flow.mmd");
    let bin = env!("CARGO_BIN_EXE_kozue");

    let tmp = std::env::temp_dir();
    let out1 = tmp.join("kozue_mmd_det_test_1.svg");
    let out2 = tmp.join("kozue_mmd_det_test_2.svg");

    let status1 = std::process::Command::new(bin)
        .args([
            "render",
            mmd.to_str().unwrap(),
            "-o",
            out1.to_str().unwrap(),
        ])
        .status()
        .expect("failed to run kozue (first run)");
    assert!(status1.success(), "first kozue run failed");

    let status2 = std::process::Command::new(bin)
        .args([
            "render",
            mmd.to_str().unwrap(),
            "-o",
            out2.to_str().unwrap(),
        ])
        .status()
        .expect("failed to run kozue (second run)");
    assert!(status2.success(), "second kozue run failed");

    let svg1 = std::fs::read(&out1).expect("read first output");
    let svg2 = std::fs::read(&out2).expect("read second output");
    let _ = std::fs::remove_file(&out1);
    let _ = std::fs::remove_file(&out2);

    assert_eq!(
        svg1, svg2,
        "same mermaid input must produce byte-identical SVG across separate process invocations"
    );
}

// ---------------------------------------------------------------------------
// PlantUML golden tests
// ---------------------------------------------------------------------------

const PLANTUML_GOLDEN_CASES: &[&str] = &[
    "plantuml_seq",
    "plantuml_seq_arrows",
    "plantuml_seq_notes",
    "plantuml_seq_dividers",
    "plantuml_seq_activation",
    "plantuml_state",
    "plantuml_class",
    "plantuml_er",
];

fn compile_plantuml(src: &str) -> String {
    let diagram = kozue_plantuml::parse(src).expect("plantuml golden input must parse");
    let scene = kozue_layout::layout(&diagram).expect("plantuml golden layout must succeed");
    kozue_render_svg::render(&scene)
}

#[test]
fn plantuml_golden_svgs_match() {
    for name in PLANTUML_GOLDEN_CASES {
        let puml = golden_dir().join(format!("{name}.puml"));
        let svg_path = golden_dir().join(format!("{name}.svg"));
        let src = std::fs::read_to_string(&puml)
            .unwrap_or_else(|e| panic!("read {}: {e}", puml.display()));
        let actual = compile_plantuml(&src);

        if std::env::var("UPDATE_GOLDEN").is_ok() {
            std::fs::write(&svg_path, &actual).unwrap();
            continue;
        }

        let expected = std::fs::read_to_string(&svg_path).unwrap_or_else(|e| {
            panic!(
                "read golden {}: {e} (run with UPDATE_GOLDEN=1 to create it)",
                svg_path.display()
            )
        });
        assert_eq!(
            actual, expected,
            "plantuml golden mismatch for {name}.puml (run with UPDATE_GOLDEN=1 to update)"
        );
    }
}

/// Verify that PlantUML rendering is deterministic across separate process invocations.
#[test]
fn plantuml_rendering_is_deterministic_across_processes() {
    let puml = golden_dir().join("plantuml_seq.puml");
    let bin = env!("CARGO_BIN_EXE_kozue");

    let tmp = std::env::temp_dir();
    let out1 = tmp.join("kozue_puml_det_test_1.svg");
    let out2 = tmp.join("kozue_puml_det_test_2.svg");

    let status1 = std::process::Command::new(bin)
        .args([
            "render",
            puml.to_str().unwrap(),
            "-o",
            out1.to_str().unwrap(),
        ])
        .status()
        .expect("failed to run kozue (first run)");
    assert!(status1.success(), "first kozue plantuml run failed");

    let status2 = std::process::Command::new(bin)
        .args([
            "render",
            puml.to_str().unwrap(),
            "-o",
            out2.to_str().unwrap(),
        ])
        .status()
        .expect("failed to run kozue (second run)");
    assert!(status2.success(), "second kozue plantuml run failed");

    let svg1 = std::fs::read(&out1).expect("read first output");
    let svg2 = std::fs::read(&out2).expect("read second output");
    let _ = std::fs::remove_file(&out1);
    let _ = std::fs::remove_file(&out2);

    assert_eq!(
        svg1, svg2,
        "same plantuml input must produce byte-identical SVG across separate process invocations"
    );
}

// ---------------------------------------------------------------------------
// Fix 4: CLI routing tests — uppercase extension and --lang override
// ---------------------------------------------------------------------------

/// Helper: write a minimal mermaid diagram to a temp file and return its path.
fn write_temp_mmd(suffix: &str, content: &str) -> std::path::PathBuf {
    let tmp = std::env::temp_dir().join(format!("kozue_routing_test{suffix}"));
    std::fs::write(&tmp, content).unwrap();
    tmp
}

const MINIMAL_MMD: &str = "flowchart TD\n  A --> B\n";
const MINIMAL_KZD: &str = "graph d {\n  a: \"A\"\n  b: \"B\"\n  a -> b\n}\n";

#[test]
fn cli_routing_uppercase_mmd_extension() {
    let bin = env!("CARGO_BIN_EXE_kozue");
    let src = write_temp_mmd(".MMD", MINIMAL_MMD);
    let out = src.with_extension("svg");
    let status = std::process::Command::new(bin)
        .args(["render", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .status()
        .expect("failed to run kozue");
    let _ = std::fs::remove_file(&src);
    let _ = std::fs::remove_file(&out);
    assert!(
        status.success(),
        ".MMD (uppercase) should route to mermaid parser"
    );
}

#[test]
fn cli_routing_uppercase_mermaid_extension() {
    let bin = env!("CARGO_BIN_EXE_kozue");
    let src = write_temp_mmd(".MERMAID", MINIMAL_MMD);
    let out = src.with_extension("svg");
    let status = std::process::Command::new(bin)
        .args(["render", src.to_str().unwrap(), "-o", out.to_str().unwrap()])
        .status()
        .expect("failed to run kozue");
    let _ = std::fs::remove_file(&src);
    let _ = std::fs::remove_file(&out);
    assert!(
        status.success(),
        ".MERMAID (uppercase) should route to mermaid parser"
    );
}

#[test]
fn cli_routing_no_extension_defaults_to_kozue() {
    let bin = env!("CARGO_BIN_EXE_kozue");
    let src = write_temp_mmd("_no_ext", MINIMAL_KZD);
    // Remove the .mmd suffix so there's no extension.
    let src_no_ext = src.with_extension("");
    // write_temp_mmd wrote to src_no_ext.mmd already (suffix="…_no_ext"), rename:
    let src_path = std::env::temp_dir().join("kozue_routing_test_no_ext");
    std::fs::write(&src_path, MINIMAL_KZD).unwrap();
    let out = src_path.with_extension("svg");
    let status = std::process::Command::new(bin)
        .args([
            "render",
            src_path.to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
        ])
        .status()
        .expect("failed to run kozue");
    let _ = std::fs::remove_file(&src_path);
    let _ = std::fs::remove_file(&out);
    let _ = std::fs::remove_file(&src);
    let _ = src_no_ext; // suppress unused warning
    assert!(
        status.success(),
        "no extension should default to kozue parser and succeed on valid kzd content"
    );
}

#[test]
fn cli_routing_lang_mermaid_override() {
    // A .kzd-named file with mermaid content rendered via --lang mermaid.
    let bin = env!("CARGO_BIN_EXE_kozue");
    let src = write_temp_mmd(".kzd", MINIMAL_MMD);
    let out = src.with_extension("svg");
    let status = std::process::Command::new(bin)
        .args([
            "render",
            src.to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
            "--lang",
            "mermaid",
        ])
        .status()
        .expect("failed to run kozue");
    let _ = std::fs::remove_file(&src);
    let _ = std::fs::remove_file(&out);
    assert!(
        status.success(),
        "--lang mermaid should override extension and use mermaid parser"
    );
}

#[test]
fn cli_routing_lang_kozue_override() {
    // A .mmd-named file with kozue content rendered via --lang kozue.
    let bin = env!("CARGO_BIN_EXE_kozue");
    let src = write_temp_mmd(".mmd", MINIMAL_KZD);
    let out = src.with_extension("svg");
    let status = std::process::Command::new(bin)
        .args([
            "render",
            src.to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
            "--lang",
            "kozue",
        ])
        .status()
        .expect("failed to run kozue");
    let _ = std::fs::remove_file(&src);
    let _ = std::fs::remove_file(&out);
    assert!(
        status.success(),
        "--lang kozue should override .mmd extension and use kozue parser"
    );
}

// ---------------------------------------------------------------------------
// M3b follow-up: CLI integration tests for kozue fmt
// ---------------------------------------------------------------------------

/// Helper: write content to a temp .kzd file and return the path.
fn write_temp_kzd(suffix: &str, content: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("kozue_fmt_test{suffix}.kzd"));
    std::fs::write(&path, content).unwrap();
    path
}

const CANONICAL_KZD: &str = "graph d {\n  a: \"A\"\n  b: \"B\"\n\n  a -> b\n}\n";
const UNFORMATTED_KZD: &str = "graph d{a:\"A\"\nb:\"B\"\na->b}\n";

#[test]
fn fmt_check_exits_nonzero_when_not_formatted() {
    let bin = env!("CARGO_BIN_EXE_kozue");
    let path = write_temp_kzd("_check_fail", UNFORMATTED_KZD);
    let status = std::process::Command::new(bin)
        .args(["fmt", "--check", path.to_str().unwrap()])
        .status()
        .expect("failed to run kozue");
    let _ = std::fs::remove_file(&path);
    assert!(
        !status.success(),
        "--check should exit non-zero when file is not formatted"
    );
}

#[test]
fn fmt_check_exits_zero_when_already_formatted() {
    let bin = env!("CARGO_BIN_EXE_kozue");
    let path = write_temp_kzd("_check_pass", CANONICAL_KZD);
    let status = std::process::Command::new(bin)
        .args(["fmt", "--check", path.to_str().unwrap()])
        .status()
        .expect("failed to run kozue");
    let _ = std::fs::remove_file(&path);
    assert!(
        status.success(),
        "--check should exit zero when file is already formatted"
    );
}

#[test]
fn fmt_inplace_no_write_when_unchanged() {
    let bin = env!("CARGO_BIN_EXE_kozue");
    let path = write_temp_kzd("_inplace_unchanged", CANONICAL_KZD);
    let mtime_before = std::fs::metadata(&path).unwrap().modified().unwrap();
    // Sleep to allow mtime to change if a write occurs.
    std::thread::sleep(std::time::Duration::from_millis(100));
    let status = std::process::Command::new(bin)
        .args(["fmt", path.to_str().unwrap()])
        .status()
        .expect("failed to run kozue");
    let content_after = std::fs::read_to_string(&path).unwrap();
    let mtime_after = std::fs::metadata(&path).unwrap().modified().unwrap();
    let _ = std::fs::remove_file(&path);
    assert!(
        status.success(),
        "fmt in-place on already-formatted file should succeed"
    );
    assert_eq!(
        content_after, CANONICAL_KZD,
        "file content must be unchanged"
    );
    assert_eq!(
        mtime_before, mtime_after,
        "file should not be rewritten when already formatted"
    );
}

#[test]
fn fmt_stdout_outputs_canonical_form() {
    let bin = env!("CARGO_BIN_EXE_kozue");
    let path = write_temp_kzd("_stdout", UNFORMATTED_KZD);
    let output = std::process::Command::new(bin)
        .args(["fmt", "--stdout", path.to_str().unwrap()])
        .output()
        .expect("failed to run kozue");
    // Verify original file is NOT modified.
    let content_after = std::fs::read_to_string(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert!(output.status.success(), "fmt --stdout should succeed");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.is_empty(), "fmt --stdout should produce output");
    assert_eq!(
        content_after, UNFORMATTED_KZD,
        "fmt --stdout must not modify the source file"
    );
}

#[test]
fn fmt_rejects_mmd_files() {
    let bin = env!("CARGO_BIN_EXE_kozue");
    let path = std::env::temp_dir().join("kozue_fmt_test_reject.mmd");
    std::fs::write(&path, "flowchart TD\n  A --> B\n").unwrap();
    let status = std::process::Command::new(bin)
        .args(["fmt", path.to_str().unwrap()])
        .status()
        .expect("failed to run kozue");
    let _ = std::fs::remove_file(&path);
    assert!(!status.success(), "fmt should reject .mmd files");
}

#[test]
fn fmt_syntax_error_does_not_modify_file() {
    let bin = env!("CARGO_BIN_EXE_kozue");
    let bad_src = "graph d { bad syntax !!! }\n";
    let path = write_temp_kzd("_syntax_err", bad_src);
    let status = std::process::Command::new(bin)
        .args(["fmt", path.to_str().unwrap()])
        .status()
        .expect("failed to run kozue");
    let content_after = std::fs::read_to_string(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert!(!status.success(), "fmt on invalid source should fail");
    assert_eq!(
        content_after, bad_src,
        "file must not be modified on syntax error"
    );
}

// ---------------------------------------------------------------------------
// M3b: Terminal renderer golden tests
// ---------------------------------------------------------------------------

fn compile_term(src: &str) -> String {
    let diagram = kozue_dsl::parse(src).expect("golden input must parse");
    let scene = kozue_layout::layout(&diagram).expect("golden layout must succeed");
    kozue_render_term::render(&scene)
}

fn compile_term_mermaid(src: &str) -> String {
    let diagram = kozue_mermaid::parse(src).expect("mermaid golden input must parse");
    let scene = kozue_layout::layout(&diagram).expect("golden layout must succeed");
    kozue_render_term::render(&scene)
}

const TERM_GOLDEN_KZD_CASES: &[&str] = &[
    "chain",
    "branch",
    "seq_basic",
    "seq_message_arrows",
    "seq_notes",
    "seq_dividers",
    "seq_activation",
    "node_shapes",
    "edge_presentation",
    "subgraph",
    "ports",
];
const TERM_GOLDEN_MMD_CASES: &[&str] = &["mermaid_flow"];

#[test]
fn term_golden_txts_match() {
    for name in TERM_GOLDEN_KZD_CASES {
        let kzd = golden_dir().join(format!("{name}.kzd"));
        let txt_path = golden_dir().join(format!("{name}.txt"));
        let src =
            std::fs::read_to_string(&kzd).unwrap_or_else(|e| panic!("read {}: {e}", kzd.display()));
        let actual = compile_term(&src);

        if std::env::var("UPDATE_GOLDEN").is_ok() {
            std::fs::write(&txt_path, &actual).unwrap();
            continue;
        }

        let expected = std::fs::read_to_string(&txt_path).unwrap_or_else(|e| {
            panic!(
                "read golden {}: {e} (run with UPDATE_GOLDEN=1 to create it)",
                txt_path.display()
            )
        });
        assert_eq!(
            actual, expected,
            "term golden mismatch for {name}.kzd (run with UPDATE_GOLDEN=1 to update)"
        );
    }
}

#[test]
fn term_mermaid_golden_txts_match() {
    for name in TERM_GOLDEN_MMD_CASES {
        let mmd = golden_dir().join(format!("{name}.mmd"));
        let txt_path = golden_dir().join(format!("{name}.txt"));
        let src =
            std::fs::read_to_string(&mmd).unwrap_or_else(|e| panic!("read {}: {e}", mmd.display()));
        let actual = compile_term_mermaid(&src);

        if std::env::var("UPDATE_GOLDEN").is_ok() {
            std::fs::write(&txt_path, &actual).unwrap();
            continue;
        }

        let expected = std::fs::read_to_string(&txt_path).unwrap_or_else(|e| {
            panic!(
                "read golden {}: {e} (run with UPDATE_GOLDEN=1 to create it)",
                txt_path.display()
            )
        });
        assert_eq!(
            actual, expected,
            "term golden mismatch for {name}.mmd (run with UPDATE_GOLDEN=1 to update)"
        );
    }
}

#[test]
fn term_render_is_deterministic() {
    let kzd = golden_dir().join("chain.kzd");
    let src = std::fs::read_to_string(&kzd).unwrap();
    let diagram = kozue_dsl::parse(&src).unwrap();
    let scene = kozue_layout::layout(&diagram).unwrap();
    let out1 = kozue_render_term::render(&scene);
    let out2 = kozue_render_term::render(&scene);
    assert_eq!(out1, out2, "terminal render must be deterministic");
}

#[test]
fn term_render_term_flag_via_cli() {
    // Smoke test: `kozue render --format term` exits 0 and produces output.
    let bin = env!("CARGO_BIN_EXE_kozue");
    let kzd = golden_dir().join("chain.kzd");
    let tmp_out = std::env::temp_dir().join("kozue_term_flag_test.txt");
    let status = std::process::Command::new(bin)
        .args([
            "render",
            "--format",
            "term",
            kzd.to_str().unwrap(),
            "-o",
            tmp_out.to_str().unwrap(),
        ])
        .status()
        .expect("failed to run kozue");
    let content = std::fs::read_to_string(&tmp_out).unwrap_or_default();
    let _ = std::fs::remove_file(&tmp_out);
    assert!(status.success(), "render --format term should succeed");
    assert!(!content.is_empty(), "term output should be non-empty");
}

// ---------------------------------------------------------------------------
// M5a: PNG golden tests and determinism
// ---------------------------------------------------------------------------

fn compile_png(src: &str) -> Vec<u8> {
    let diagram = kozue_dsl::parse(src).expect("golden input must parse");
    let scene = kozue_layout::layout(&diagram).expect("golden layout must succeed");
    kozue_render_png::render(&scene).expect("golden PNG render must succeed")
}

const PNG_GOLDEN_CASES: &[&str] = &[
    "chain",
    "branch",
    "seq_basic",
    "seq_message_arrows",
    "seq_notes",
    "seq_dividers",
    "seq_activation",
    "node_shapes",
    "edge_presentation",
    "subgraph",
    "ports",
];

#[test]
fn golden_pngs_match() {
    for name in PNG_GOLDEN_CASES {
        let kzd = golden_dir().join(format!("{name}.kzd"));
        let png_path = golden_dir().join(format!("{name}.png"));
        let src =
            std::fs::read_to_string(&kzd).unwrap_or_else(|e| panic!("read {}: {e}", kzd.display()));
        let actual = compile_png(&src);

        // Allow regenerating goldens with UPDATE_GOLDEN=1.
        if std::env::var("UPDATE_GOLDEN").is_ok() {
            std::fs::write(&png_path, &actual).unwrap();
            continue;
        }

        let expected = std::fs::read(&png_path).unwrap_or_else(|e| {
            panic!(
                "read golden {}: {e} (run with UPDATE_GOLDEN=1 to create it)",
                png_path.display()
            )
        });
        assert_eq!(
            actual, expected,
            "golden PNG mismatch for {name}.kzd (run with UPDATE_GOLDEN=1 to update)"
        );
    }
}

#[test]
fn png_rendering_is_deterministic_across_processes() {
    let kzd = golden_dir().join("branch.kzd");
    let bin = env!("CARGO_BIN_EXE_kozue");

    let tmp = std::env::temp_dir();
    let out1 = tmp.join("kozue_png_det_test_1.png");
    let out2 = tmp.join("kozue_png_det_test_2.png");

    let status1 = std::process::Command::new(bin)
        .args([
            "render",
            "--format",
            "png",
            kzd.to_str().unwrap(),
            "-o",
            out1.to_str().unwrap(),
        ])
        .status()
        .expect("failed to run kozue (first run)");
    assert!(status1.success(), "first kozue PNG run failed");

    let status2 = std::process::Command::new(bin)
        .args([
            "render",
            "--format",
            "png",
            kzd.to_str().unwrap(),
            "-o",
            out2.to_str().unwrap(),
        ])
        .status()
        .expect("failed to run kozue (second run)");
    assert!(status2.success(), "second kozue PNG run failed");

    let png1 = std::fs::read(&out1).expect("read first output");
    let png2 = std::fs::read(&out2).expect("read second output");
    let _ = std::fs::remove_file(&out1);
    let _ = std::fs::remove_file(&out2);

    assert_eq!(
        png1, png2,
        "same input must produce byte-identical PNG across separate process invocations"
    );

    // Tie the cross-process output back to the committed golden so a stable but
    // wrong regression is caught here too, not only by the in-process test.
    let golden = std::fs::read(golden_dir().join("branch.png")).expect("read branch.png golden");
    assert_eq!(
        png1, golden,
        "CLI PNG output must match the committed branch.png golden"
    );
}

// ---------------------------------------------------------------------------
// M7a: State diagram golden tests
// ---------------------------------------------------------------------------

const STATE_GOLDEN_CASES: &[&str] = &["state_basic", "state_bidirectional"];

fn compile_state(src: &str) -> String {
    let diagram = kozue_dsl::parse(src).expect("state golden input must parse");
    let scene = kozue_layout::layout(&diagram).expect("state golden layout must succeed");
    kozue_render_svg::render(&scene)
}

#[test]
fn state_golden_svgs_match() {
    for name in STATE_GOLDEN_CASES {
        let kzd = golden_dir().join(format!("{name}.kzd"));
        let svg_path = golden_dir().join(format!("{name}.svg"));
        let src =
            std::fs::read_to_string(&kzd).unwrap_or_else(|e| panic!("read {}: {e}", kzd.display()));
        let actual = compile_state(&src);

        if std::env::var("UPDATE_GOLDEN").is_ok() {
            std::fs::write(&svg_path, &actual).unwrap();
            continue;
        }

        let expected = std::fs::read_to_string(&svg_path).unwrap_or_else(|e| {
            panic!(
                "read golden {}: {e} (run with UPDATE_GOLDEN=1 to create it)",
                svg_path.display()
            )
        });
        assert_eq!(
            actual, expected,
            "golden mismatch for {name}.kzd (run with UPDATE_GOLDEN=1 to update)"
        );
    }
}

#[test]
fn state_golden_pngs_match() {
    for name in STATE_GOLDEN_CASES {
        let kzd = golden_dir().join(format!("{name}.kzd"));
        let png_path = golden_dir().join(format!("{name}.png"));
        let src =
            std::fs::read_to_string(&kzd).unwrap_or_else(|e| panic!("read {}: {e}", kzd.display()));
        let actual = {
            let diagram = kozue_dsl::parse(&src).expect("state golden input must parse");
            let scene = kozue_layout::layout(&diagram).expect("state golden layout must succeed");
            kozue_render_png::render(&scene).expect("state golden PNG render must succeed")
        };

        if std::env::var("UPDATE_GOLDEN").is_ok() {
            std::fs::write(&png_path, &actual).unwrap();
            continue;
        }

        let expected = std::fs::read(&png_path).unwrap_or_else(|e| {
            panic!(
                "read golden {}: {e} (run with UPDATE_GOLDEN=1 to create it)",
                png_path.display()
            )
        });
        assert_eq!(
            actual, expected,
            "golden PNG mismatch for {name}.kzd (run with UPDATE_GOLDEN=1 to update)"
        );
    }
}

#[test]
fn state_golden_term_match() {
    for name in STATE_GOLDEN_CASES {
        let kzd = golden_dir().join(format!("{name}.kzd"));
        let txt_path = golden_dir().join(format!("{name}.txt"));
        let src =
            std::fs::read_to_string(&kzd).unwrap_or_else(|e| panic!("read {}: {e}", kzd.display()));
        let actual = {
            let diagram = kozue_dsl::parse(&src).expect("state golden input must parse");
            let scene = kozue_layout::layout(&diagram).expect("state golden layout must succeed");
            kozue_render_term::render(&scene)
        };

        if std::env::var("UPDATE_GOLDEN").is_ok() {
            std::fs::write(&txt_path, &actual).unwrap();
            continue;
        }

        let expected = std::fs::read_to_string(&txt_path).unwrap_or_else(|e| {
            panic!(
                "read golden {}: {e} (run with UPDATE_GOLDEN=1 to create it)",
                txt_path.display()
            )
        });
        assert_eq!(
            actual, expected,
            "term golden mismatch for {name}.kzd (run with UPDATE_GOLDEN=1 to update)"
        );
    }
}

#[test]
fn state_rendering_is_deterministic() {
    let kzd = golden_dir().join("state_basic.kzd");
    let src = std::fs::read_to_string(&kzd).unwrap();
    let diagram = kozue_dsl::parse(&src).unwrap();
    let scene1 = kozue_layout::layout(&diagram).unwrap();
    let scene2 = kozue_layout::layout(&diagram).unwrap();
    let svg1 = kozue_render_svg::render(&scene1);
    let svg2 = kozue_render_svg::render(&scene2);
    assert_eq!(svg1, svg2, "state rendering must be deterministic");
}

// ---------------------------------------------------------------------------
// Phase B: class / ER diagram golden tests (native DSL frontend)
// ---------------------------------------------------------------------------

const CLASS_GOLDEN_CASES: &[&str] = &["class_basic"];
const ER_GOLDEN_CASES: &[&str] = &["er_basic"];

/// SVG golden match for the native-DSL class and ER diagrams.
#[test]
fn class_er_golden_svgs_match() {
    for name in CLASS_GOLDEN_CASES.iter().chain(ER_GOLDEN_CASES.iter()) {
        let kzd = golden_dir().join(format!("{name}.kzd"));
        let svg_path = golden_dir().join(format!("{name}.svg"));
        let src =
            std::fs::read_to_string(&kzd).unwrap_or_else(|e| panic!("read {}: {e}", kzd.display()));
        let actual = compile(&src);

        if std::env::var("UPDATE_GOLDEN").is_ok() {
            std::fs::write(&svg_path, &actual).unwrap();
            continue;
        }

        let expected = std::fs::read_to_string(&svg_path).unwrap_or_else(|e| {
            panic!(
                "read golden {}: {e} (run with UPDATE_GOLDEN=1 to create it)",
                svg_path.display()
            )
        });
        assert_eq!(
            actual, expected,
            "class/ER golden mismatch for {name}.kzd (run with UPDATE_GOLDEN=1 to update)"
        );
    }
}

/// Term (text) golden match for the native-DSL class and ER diagrams.
#[test]
fn class_er_golden_term_match() {
    for name in CLASS_GOLDEN_CASES.iter().chain(ER_GOLDEN_CASES.iter()) {
        let kzd = golden_dir().join(format!("{name}.kzd"));
        let txt_path = golden_dir().join(format!("{name}.txt"));
        let src =
            std::fs::read_to_string(&kzd).unwrap_or_else(|e| panic!("read {}: {e}", kzd.display()));
        let actual = {
            let diagram = kozue_dsl::parse(&src).expect("class/ER golden input must parse");
            let scene =
                kozue_layout::layout(&diagram).expect("class/ER golden layout must succeed");
            kozue_render_term::render(&scene)
        };

        if std::env::var("UPDATE_GOLDEN").is_ok() {
            std::fs::write(&txt_path, &actual).unwrap();
            continue;
        }

        let expected = std::fs::read_to_string(&txt_path).unwrap_or_else(|e| {
            panic!(
                "read golden {}: {e} (run with UPDATE_GOLDEN=1 to create it)",
                txt_path.display()
            )
        });
        assert_eq!(
            actual, expected,
            "class/ER term golden mismatch for {name}.kzd (run with UPDATE_GOLDEN=1 to update)"
        );
    }
}

/// PNG golden match for the native-DSL class and ER diagrams.
#[test]
fn class_er_golden_pngs_match() {
    for name in CLASS_GOLDEN_CASES.iter().chain(ER_GOLDEN_CASES.iter()) {
        let kzd = golden_dir().join(format!("{name}.kzd"));
        let png_path = golden_dir().join(format!("{name}.png"));
        let src =
            std::fs::read_to_string(&kzd).unwrap_or_else(|e| panic!("read {}: {e}", kzd.display()));
        let actual = compile_png(&src);

        if std::env::var("UPDATE_GOLDEN").is_ok() {
            std::fs::write(&png_path, &actual).unwrap();
            continue;
        }

        let expected = std::fs::read(&png_path).unwrap_or_else(|e| {
            panic!(
                "read golden {}: {e} (run with UPDATE_GOLDEN=1 to create it)",
                png_path.display()
            )
        });
        assert_eq!(
            actual, expected,
            "class/ER PNG golden mismatch for {name}.kzd (run with UPDATE_GOLDEN=1 to update)"
        );
    }
}

/// Verify that class-diagram rendering is deterministic across separate process
/// invocations (guards against HashMap-seed or other process-level randomness).
#[test]
fn class_rendering_is_deterministic_across_processes() {
    let kzd = golden_dir().join("class_basic.kzd");
    let bin = env!("CARGO_BIN_EXE_kozue");

    let tmp = std::env::temp_dir();
    let out1 = tmp.join("kozue_class_det_test_1.svg");
    let out2 = tmp.join("kozue_class_det_test_2.svg");

    let status1 = std::process::Command::new(bin)
        .args([
            "render",
            kzd.to_str().unwrap(),
            "-o",
            out1.to_str().unwrap(),
        ])
        .status()
        .expect("failed to run kozue (first run)");
    assert!(status1.success(), "first kozue class run failed");

    let status2 = std::process::Command::new(bin)
        .args([
            "render",
            kzd.to_str().unwrap(),
            "-o",
            out2.to_str().unwrap(),
        ])
        .status()
        .expect("failed to run kozue (second run)");
    assert!(status2.success(), "second kozue class run failed");

    let svg1 = std::fs::read(&out1).expect("read first output");
    let svg2 = std::fs::read(&out2).expect("read second output");
    let _ = std::fs::remove_file(&out1);
    let _ = std::fs::remove_file(&out2);

    assert_eq!(
        svg1, svg2,
        "same class input must produce byte-identical SVG across separate process invocations"
    );
}

// ---------------------------------------------------------------------------
// M8b: draw.io golden tests
// ---------------------------------------------------------------------------

fn compile_drawio_kzd(src: &str) -> String {
    let diagram = kozue_dsl::parse(src).expect("golden input must parse");
    let layout_out = kozue_layout::layout_full(&diagram).expect("golden layout must succeed");
    kozue_render_drawio::render(&layout_out.semantic).expect("golden draw.io render must succeed")
}

const DRAWIO_GRAPH_GOLDEN_CASES: &[&str] = &[
    "chain",
    "branch",
    "skip",
    "node_shapes",
    "edge_presentation",
    "subgraph",
    "ports",
];
const DRAWIO_STATE_GOLDEN_CASES: &[&str] = &["state_basic", "state_bidirectional"];
const DRAWIO_SEQUENCE_GOLDEN_CASES: &[&str] = &[
    "seq_minimal",
    "seq_basic",
    "seq_self_dashed",
    "seq_participant_kinds",
    "seq_message_arrows",
    "seq_notes",
    "seq_dividers",
    "seq_activation",
];
const DRAWIO_CLASS_GOLDEN_CASES: &[&str] = &["class_basic"];
const DRAWIO_ER_GOLDEN_CASES: &[&str] = &["er_basic"];

#[test]
fn drawio_graph_goldens_match() {
    for name in DRAWIO_GRAPH_GOLDEN_CASES {
        let kzd = golden_dir().join(format!("{name}.kzd"));
        let drawio_path = golden_dir().join(format!("{name}.drawio"));
        let src =
            std::fs::read_to_string(&kzd).unwrap_or_else(|e| panic!("read {}: {e}", kzd.display()));
        let actual = compile_drawio_kzd(&src);

        if std::env::var("UPDATE_GOLDEN").is_ok() {
            std::fs::write(&drawio_path, &actual).unwrap();
            continue;
        }

        let expected = std::fs::read_to_string(&drawio_path).unwrap_or_else(|e| {
            panic!(
                "read golden {}: {e} (run with UPDATE_GOLDEN=1 to create it)",
                drawio_path.display()
            )
        });
        assert_eq!(
            actual, expected,
            "draw.io golden mismatch for {name}.kzd (run with UPDATE_GOLDEN=1 to update)"
        );
    }
}

#[test]
fn drawio_state_goldens_match() {
    for name in DRAWIO_STATE_GOLDEN_CASES {
        let kzd = golden_dir().join(format!("{name}.kzd"));
        let drawio_path = golden_dir().join(format!("{name}.drawio"));
        let src =
            std::fs::read_to_string(&kzd).unwrap_or_else(|e| panic!("read {}: {e}", kzd.display()));
        let actual = compile_drawio_kzd(&src);

        if std::env::var("UPDATE_GOLDEN").is_ok() {
            std::fs::write(&drawio_path, &actual).unwrap();
            continue;
        }

        let expected = std::fs::read_to_string(&drawio_path).unwrap_or_else(|e| {
            panic!(
                "read golden {}: {e} (run with UPDATE_GOLDEN=1 to create it)",
                drawio_path.display()
            )
        });
        assert_eq!(
            actual, expected,
            "draw.io golden mismatch for {name}.kzd (run with UPDATE_GOLDEN=1 to update)"
        );
    }
}

#[test]
fn drawio_class_er_goldens_match() {
    for name in DRAWIO_CLASS_GOLDEN_CASES
        .iter()
        .chain(DRAWIO_ER_GOLDEN_CASES.iter())
    {
        let kzd = golden_dir().join(format!("{name}.kzd"));
        let drawio_path = golden_dir().join(format!("{name}.drawio"));
        let src =
            std::fs::read_to_string(&kzd).unwrap_or_else(|e| panic!("read {}: {e}", kzd.display()));
        let actual = compile_drawio_kzd(&src);

        if std::env::var("UPDATE_GOLDEN").is_ok() {
            std::fs::write(&drawio_path, &actual).unwrap();
            continue;
        }

        let expected = std::fs::read_to_string(&drawio_path).unwrap_or_else(|e| {
            panic!(
                "read golden {}: {e} (run with UPDATE_GOLDEN=1 to create it)",
                drawio_path.display()
            )
        });
        assert_eq!(
            actual, expected,
            "draw.io golden mismatch for {name}.kzd (run with UPDATE_GOLDEN=1 to update)"
        );
    }
}

#[test]
fn drawio_render_is_deterministic() {
    let src = std::fs::read_to_string(golden_dir().join("chain.kzd")).unwrap();
    let diagram = kozue_dsl::parse(&src).unwrap();
    let out1 = kozue_layout::layout_full(&diagram).unwrap();
    let out2 = kozue_layout::layout_full(&diagram).unwrap();
    let xml1 = kozue_render_drawio::render(&out1.semantic).unwrap();
    let xml2 = kozue_render_drawio::render(&out2.semantic).unwrap();
    assert_eq!(xml1, xml2, "draw.io render must be deterministic");
}

#[test]
fn drawio_graph_edge_emits_waypoints_for_multilayer_edge() {
    // skip.kzd has a -> d spanning three layers; its draw.io edge must carry an
    // <Array as="points"> with the interior route points (route[1..n-1]).
    let src = std::fs::read_to_string(golden_dir().join("skip.kzd")).unwrap();
    let diagram = kozue_dsl::parse(&src).unwrap();
    let out = kozue_layout::layout_full(&diagram).unwrap();
    let xml = kozue_render_drawio::render(&out.semantic).unwrap();
    assert!(
        xml.contains("<Array as=\"points\">"),
        "multi-layer graph edge must emit a waypoint Array: {xml}"
    );
    assert!(
        xml.contains("<mxPoint "),
        "waypoint Array must contain mxPoint children: {xml}"
    );
}

#[test]
fn drawio_sequence_goldens_match() {
    for name in DRAWIO_SEQUENCE_GOLDEN_CASES {
        let kzd = golden_dir().join(format!("{name}.kzd"));
        let drawio_path = golden_dir().join(format!("{name}.drawio"));
        let src =
            std::fs::read_to_string(&kzd).unwrap_or_else(|e| panic!("read {}: {e}", kzd.display()));
        let actual = compile_drawio_kzd(&src);

        if std::env::var("UPDATE_GOLDEN").is_ok() {
            std::fs::write(&drawio_path, &actual).unwrap();
            continue;
        }

        let expected = std::fs::read_to_string(&drawio_path).unwrap_or_else(|e| {
            panic!(
                "read golden {}: {e} (run with UPDATE_GOLDEN=1 to create it)",
                drawio_path.display()
            )
        });
        assert_eq!(
            actual, expected,
            "draw.io golden mismatch for {name}.kzd (run with UPDATE_GOLDEN=1 to update)"
        );
    }
}

/// Extract a fractional style value (e.g. `exitY`) from the style of the
/// `<mxCell id="{cell_id}" ...>` element in rendered draw.io XML.
fn drawio_style_frac(xml: &str, cell_id: &str, key: &str) -> f64 {
    let open = format!("<mxCell id=\"{cell_id}\"");
    let cell_start = xml
        .find(&open)
        .unwrap_or_else(|| panic!("no cell {cell_id}"));
    let cell = &xml[cell_start..xml[cell_start..].find('>').unwrap() + cell_start];
    let needle = format!("{key}=");
    let val_start = cell
        .find(&needle)
        .unwrap_or_else(|| panic!("no {key} in style of {cell_id}: {cell}"))
        + needle.len();
    let rest = &cell[val_start..];
    let val = &rest[..rest.find(';').unwrap_or(rest.len())];
    val.parse()
        .unwrap_or_else(|e| panic!("bad {key} value {val:?} in {cell_id}: {e}"))
}

/// Contract test guarding the *rendered* exitY/entryY pins: for every message,
/// the fraction emitted in the draw.io XML, applied to the lifeline vertex
/// geometry as draw.io would (frac × vertex_height + vertex_top), must
/// reconstruct the message's semantic y within 0.1 px. Unlike the goldens,
/// this cannot be blessed away by UPDATE_GOLDEN: it catches a formatter
/// regression (e.g. dropping to 2 decimals) or a wrong denominator (e.g. the
/// lifeline span instead of the full vertex height) in the renderer itself.
#[test]
fn drawio_sequence_pins_preserve_message_y() {
    use kozue_layout::semantic::{SemanticLayout, SequenceItemLayout};
    for name in DRAWIO_SEQUENCE_GOLDEN_CASES {
        let src = std::fs::read_to_string(golden_dir().join(format!("{name}.kzd"))).unwrap();
        let diagram = kozue_dsl::parse(&src).unwrap();
        let out = kozue_layout::layout_full(&diagram).unwrap();
        let SemanticLayout::Sequence(s) = &out.semantic else {
            panic!("{name} must be a sequence layout");
        };
        let xml = kozue_render_drawio::render(&out.semantic).unwrap();
        for (i, item) in s.items.iter().enumerate() {
            let SequenceItemLayout::Message(m) = item else {
                continue;
            };
            let src_p = s.participants.iter().find(|p| p.id == m.from).unwrap();
            let tgt_p = s.participants.iter().find(|p| p.id == m.to).unwrap();
            let cell_id = format!("e{i}");
            for (key, p, y) in [
                ("exitY", src_p, m.route.first().unwrap().y),
                ("entryY", tgt_p, m.route.last().unwrap().y),
            ] {
                let frac = drawio_style_frac(&xml, &cell_id, key);
                // Reconstruct the y as draw.io would, from the vertex geometry.
                let h = p.lifeline_y1 - p.header_rect.y;
                let reconstructed = frac * h + p.header_rect.y;
                assert!(
                    (reconstructed - y).abs() < 0.1,
                    "{name}: rendered {key} of {cell_id} desyncs y: \
                     got {reconstructed}, want {y}"
                );
            }
        }
    }
}

#[test]
fn drawio_sequence_self_message_is_self_loop_with_waypoints() {
    // seq_self_dashed has `a -> a` and `b -->> b`; each must be a self-loop
    // (source == target) carrying fold waypoints.
    use kozue_layout::semantic::SemanticLayout;
    let src = std::fs::read_to_string(golden_dir().join("seq_self_dashed.kzd")).unwrap();
    let diagram = kozue_dsl::parse(&src).unwrap();
    let out = kozue_layout::layout_full(&diagram).unwrap();
    let SemanticLayout::Sequence(s) = &out.semantic else {
        panic!("expected sequence");
    };
    let self_msgs = s
        .items
        .iter()
        .filter_map(|it| match it {
            kozue_layout::semantic::SequenceItemLayout::Message(m) => Some(m),
            _ => None,
        })
        .filter(|m| m.from == m.to)
        .count();
    assert_eq!(self_msgs, 2, "two self-messages expected");
    let xml = kozue_render_drawio::render(&out.semantic).unwrap();
    // A self-loop connects a lifeline to itself and carries a waypoint Array.
    assert!(
        xml.contains("source=\"n0\" target=\"n0\"") || xml.contains("source=\"n1\" target=\"n1\""),
        "self-message must be a self-loop edge: {xml}"
    );
    assert!(
        xml.contains("<Array as=\"points\">"),
        "self-message must carry fold waypoints: {xml}"
    );
    // The self-loop label lives in a child edgeLabel cell (so it follows the loop
    // on drag), not inline in the edge value.
    assert!(
        xml.contains("style=\"edgeLabel;") && xml.contains("connectable=\"0\""),
        "self-message label must be a child edgeLabel cell: {xml}"
    );
    assert!(
        xml.contains("parent=\"e2\"") || xml.contains("parent=\"e3\""),
        "label cell must be parented to its self-loop edge: {xml}"
    );
}

/// Guard the B-class "follow" guarantee: a straight (non-self) message must NOT
/// carry absolute waypoints. If a future layout change starts adding interior
/// route points to straight messages, those absolute mxPoints would be left
/// behind when a participant is dragged — silently breaking connection-follow.
/// seq_minimal is a single straight message, so its edge must be waypoint-free.
#[test]
fn drawio_straight_message_has_no_absolute_waypoints() {
    let src = std::fs::read_to_string(golden_dir().join("seq_minimal.kzd")).unwrap();
    let diagram = kozue_dsl::parse(&src).unwrap();
    let out = kozue_layout::layout_full(&diagram).unwrap();
    let xml = kozue_render_drawio::render(&out.semantic).unwrap();
    assert!(
        !xml.contains("<Array as=\"points\">"),
        "straight message must not emit absolute waypoints (would break follow-on-move): {xml}"
    );
}

#[test]
fn drawio_cli_flag_produces_output() {
    let bin = env!("CARGO_BIN_EXE_kozue");
    let kzd = golden_dir().join("chain.kzd");
    let tmp_out = std::env::temp_dir().join("kozue_drawio_flag_test.drawio");
    let status = std::process::Command::new(bin)
        .args([
            "render",
            "--format",
            "drawio",
            kzd.to_str().unwrap(),
            "-o",
            tmp_out.to_str().unwrap(),
        ])
        .status()
        .expect("failed to run kozue");
    let content = std::fs::read_to_string(&tmp_out).unwrap_or_default();
    let _ = std::fs::remove_file(&tmp_out);
    assert!(status.success(), "render --format drawio should succeed");
    assert!(!content.is_empty(), "draw.io output should be non-empty");
    assert!(content.contains("<mxfile>"), "output must be mxfile XML");
}

// ---------------------------------------------------------------------------
// Graphviz DOT golden tests
// ---------------------------------------------------------------------------

fn compile_dot_kzd(src: &str) -> String {
    let diagram = kozue_dsl::parse(src).expect("golden input must parse");
    kozue_render_dot::render(&diagram).expect("golden DOT render must succeed")
}

const DOT_GRAPH_GOLDEN_CASES: &[&str] = &[
    "chain",
    "branch",
    "right",
    "cycle",
    "skip",
    "wide_right",
    "node_shapes",
    "edge_presentation",
    "subgraph",
    "ports",
];
const DOT_STATE_GOLDEN_CASES: &[&str] = &["state_basic", "state_bidirectional"];
const DOT_CLASS_GOLDEN_CASES: &[&str] = &["class_basic"];
const DOT_ER_GOLDEN_CASES: &[&str] = &["er_basic"];

#[test]
fn dot_goldens_match() {
    let cases = DOT_GRAPH_GOLDEN_CASES
        .iter()
        .chain(DOT_STATE_GOLDEN_CASES.iter())
        .chain(DOT_CLASS_GOLDEN_CASES.iter())
        .chain(DOT_ER_GOLDEN_CASES.iter());
    for name in cases {
        let kzd = golden_dir().join(format!("{name}.kzd"));
        let dot_path = golden_dir().join(format!("{name}.dot"));
        let src =
            std::fs::read_to_string(&kzd).unwrap_or_else(|e| panic!("read {}: {e}", kzd.display()));
        let actual = compile_dot_kzd(&src);

        if std::env::var("UPDATE_GOLDEN").is_ok() {
            std::fs::write(&dot_path, &actual).unwrap();
            continue;
        }

        let expected = std::fs::read_to_string(&dot_path).unwrap_or_else(|e| {
            panic!(
                "read golden {}: {e} (run with UPDATE_GOLDEN=1 to create it)",
                dot_path.display()
            )
        });
        assert_eq!(
            actual, expected,
            "DOT golden mismatch for {name}.kzd (run with UPDATE_GOLDEN=1 to update)"
        );
    }
}

#[test]
fn dot_render_is_deterministic() {
    let src = std::fs::read_to_string(golden_dir().join("branch.kzd")).unwrap();
    let d1 = kozue_dsl::parse(&src).unwrap();
    let d2 = kozue_dsl::parse(&src).unwrap();
    assert_eq!(
        kozue_render_dot::render(&d1).unwrap(),
        kozue_render_dot::render(&d2).unwrap(),
        "DOT rendering must be deterministic"
    );
}

#[test]
fn dot_sequence_is_unsupported() {
    let src = std::fs::read_to_string(golden_dir().join("seq_basic.kzd")).unwrap();
    let diagram = kozue_dsl::parse(&src).unwrap();
    assert!(
        kozue_render_dot::render(&diagram).is_err(),
        "sequence diagrams have no DOT representation and must error"
    );
}

#[test]
fn dot_cli_flag_produces_output() {
    let bin = env!("CARGO_BIN_EXE_kozue");
    let kzd = golden_dir().join("chain.kzd");
    let tmp_out = std::env::temp_dir().join("kozue_dot_flag_test.dot");
    let status = std::process::Command::new(bin)
        .args([
            "render",
            "--format",
            "dot",
            kzd.to_str().unwrap(),
            "-o",
            tmp_out.to_str().unwrap(),
        ])
        .status()
        .expect("failed to run kozue");
    let content = std::fs::read_to_string(&tmp_out).unwrap_or_default();
    let _ = std::fs::remove_file(&tmp_out);
    assert!(status.success(), "render --format dot should succeed");
    assert!(
        content.starts_with("digraph {"),
        "output must be a DOT digraph"
    );
}

// ---------------------------------------------------------------------------
// M10: Excalidraw golden tests
// ---------------------------------------------------------------------------

fn compile_excalidraw_kzd(src: &str) -> String {
    let diagram = kozue_dsl::parse(src).expect("golden input must parse");
    let layout_out = kozue_layout::layout_full(&diagram).expect("golden layout must succeed");
    kozue_render_excalidraw::render(&layout_out.semantic)
        .expect("golden Excalidraw render must succeed")
}

const EXCALIDRAW_GRAPH_GOLDEN_CASES: &[&str] = &[
    "chain",
    "branch",
    "skip",
    "node_shapes",
    "edge_presentation",
    "subgraph",
    "ports",
];
const EXCALIDRAW_STATE_GOLDEN_CASES: &[&str] = &["state_basic", "state_bidirectional"];
const EXCALIDRAW_SEQUENCE_GOLDEN_CASES: &[&str] = &[
    "seq_minimal",
    "seq_basic",
    "seq_self_dashed",
    "seq_participant_kinds",
    "seq_message_arrows",
    "seq_notes",
    "seq_dividers",
    "seq_activation",
];
const EXCALIDRAW_CLASS_GOLDEN_CASES: &[&str] = &["class_basic"];
const EXCALIDRAW_ER_GOLDEN_CASES: &[&str] = &["er_basic"];

#[test]
fn excalidraw_graph_goldens_match() {
    for name in EXCALIDRAW_GRAPH_GOLDEN_CASES {
        let kzd = golden_dir().join(format!("{name}.kzd"));
        let excalidraw_path = golden_dir().join(format!("{name}.excalidraw"));
        let src =
            std::fs::read_to_string(&kzd).unwrap_or_else(|e| panic!("read {}: {e}", kzd.display()));
        let actual = compile_excalidraw_kzd(&src);

        if std::env::var("UPDATE_GOLDEN").is_ok() {
            std::fs::write(&excalidraw_path, &actual).unwrap();
            continue;
        }

        let expected = std::fs::read_to_string(&excalidraw_path).unwrap_or_else(|e| {
            panic!(
                "read golden {}: {e} (run with UPDATE_GOLDEN=1 to create it)",
                excalidraw_path.display()
            )
        });
        assert_eq!(
            actual, expected,
            "Excalidraw golden mismatch for {name}.kzd (run with UPDATE_GOLDEN=1 to update)"
        );
    }
}

#[test]
fn excalidraw_state_goldens_match() {
    for name in EXCALIDRAW_STATE_GOLDEN_CASES {
        let kzd = golden_dir().join(format!("{name}.kzd"));
        let excalidraw_path = golden_dir().join(format!("{name}.excalidraw"));
        let src =
            std::fs::read_to_string(&kzd).unwrap_or_else(|e| panic!("read {}: {e}", kzd.display()));
        let actual = compile_excalidraw_kzd(&src);

        if std::env::var("UPDATE_GOLDEN").is_ok() {
            std::fs::write(&excalidraw_path, &actual).unwrap();
            continue;
        }

        let expected = std::fs::read_to_string(&excalidraw_path).unwrap_or_else(|e| {
            panic!(
                "read golden {}: {e} (run with UPDATE_GOLDEN=1 to create it)",
                excalidraw_path.display()
            )
        });
        assert_eq!(
            actual, expected,
            "Excalidraw golden mismatch for {name}.kzd (run with UPDATE_GOLDEN=1 to update)"
        );
    }
}

#[test]
fn excalidraw_sequence_goldens_match() {
    for name in EXCALIDRAW_SEQUENCE_GOLDEN_CASES {
        let kzd = golden_dir().join(format!("{name}.kzd"));
        let excalidraw_path = golden_dir().join(format!("{name}.excalidraw"));
        let src =
            std::fs::read_to_string(&kzd).unwrap_or_else(|e| panic!("read {}: {e}", kzd.display()));
        let actual = compile_excalidraw_kzd(&src);

        if std::env::var("UPDATE_GOLDEN").is_ok() {
            std::fs::write(&excalidraw_path, &actual).unwrap();
            continue;
        }

        let expected = std::fs::read_to_string(&excalidraw_path).unwrap_or_else(|e| {
            panic!(
                "read golden {}: {e} (run with UPDATE_GOLDEN=1 to create it)",
                excalidraw_path.display()
            )
        });
        assert_eq!(
            actual, expected,
            "Excalidraw golden mismatch for {name}.kzd (run with UPDATE_GOLDEN=1 to update)"
        );
    }
}

#[test]
fn excalidraw_class_er_goldens_match() {
    for name in EXCALIDRAW_CLASS_GOLDEN_CASES
        .iter()
        .chain(EXCALIDRAW_ER_GOLDEN_CASES.iter())
    {
        let kzd = golden_dir().join(format!("{name}.kzd"));
        let excalidraw_path = golden_dir().join(format!("{name}.excalidraw"));
        let src =
            std::fs::read_to_string(&kzd).unwrap_or_else(|e| panic!("read {}: {e}", kzd.display()));
        let actual = compile_excalidraw_kzd(&src);

        if std::env::var("UPDATE_GOLDEN").is_ok() {
            std::fs::write(&excalidraw_path, &actual).unwrap();
            continue;
        }

        let expected = std::fs::read_to_string(&excalidraw_path).unwrap_or_else(|e| {
            panic!(
                "read golden {}: {e} (run with UPDATE_GOLDEN=1 to create it)",
                excalidraw_path.display()
            )
        });
        assert_eq!(
            actual, expected,
            "Excalidraw golden mismatch for {name}.kzd (run with UPDATE_GOLDEN=1 to update)"
        );
    }
}

#[test]
fn excalidraw_render_is_deterministic() {
    let src = std::fs::read_to_string(golden_dir().join("chain.kzd")).unwrap();
    let diagram = kozue_dsl::parse(&src).unwrap();
    let out1 = kozue_layout::layout_full(&diagram).unwrap();
    let out2 = kozue_layout::layout_full(&diagram).unwrap();
    let json1 = kozue_render_excalidraw::render(&out1.semantic).unwrap();
    let json2 = kozue_render_excalidraw::render(&out2.semantic).unwrap();
    assert_eq!(json1, json2, "Excalidraw render must be deterministic");
}

/// Every Excalidraw golden must be valid JSON that round-trips through
/// `serde_json::Value` and declares the expected top-level envelope.
#[test]
fn excalidraw_goldens_are_well_formed_json() {
    let cases = EXCALIDRAW_GRAPH_GOLDEN_CASES
        .iter()
        .chain(EXCALIDRAW_STATE_GOLDEN_CASES.iter())
        .chain(EXCALIDRAW_SEQUENCE_GOLDEN_CASES.iter())
        .chain(EXCALIDRAW_CLASS_GOLDEN_CASES.iter())
        .chain(EXCALIDRAW_ER_GOLDEN_CASES.iter());
    for name in cases {
        let excalidraw_path = golden_dir().join(format!("{name}.excalidraw"));
        let content = std::fs::read_to_string(&excalidraw_path)
            .unwrap_or_else(|e| panic!("read golden {}: {e}", excalidraw_path.display()));
        let value: serde_json::Value = serde_json::from_str(&content)
            .unwrap_or_else(|e| panic!("{name}.excalidraw is not valid JSON: {e}"));
        assert_eq!(
            value["type"], "excalidraw",
            "{name}.excalidraw must declare type=excalidraw"
        );
        assert_eq!(
            value["version"], 2,
            "{name}.excalidraw must declare version=2"
        );
        assert!(
            value["elements"].as_array().is_some_and(|a| !a.is_empty()),
            "{name}.excalidraw must have a non-empty elements array"
        );
    }
}

#[test]
fn excalidraw_cli_flag_produces_output() {
    let bin = env!("CARGO_BIN_EXE_kozue");
    let kzd = golden_dir().join("chain.kzd");
    let tmp_out = std::env::temp_dir().join("kozue_excalidraw_flag_test.excalidraw");
    let status = std::process::Command::new(bin)
        .args([
            "render",
            "--format",
            "excalidraw",
            kzd.to_str().unwrap(),
            "-o",
            tmp_out.to_str().unwrap(),
        ])
        .status()
        .expect("failed to run kozue");
    let content = std::fs::read_to_string(&tmp_out).unwrap_or_default();
    let _ = std::fs::remove_file(&tmp_out);
    assert!(
        status.success(),
        "render --format excalidraw should succeed"
    );
    assert!(!content.is_empty(), "Excalidraw output should be non-empty");
    let value: serde_json::Value =
        serde_json::from_str(&content).expect("output must be valid JSON");
    assert_eq!(
        value["type"], "excalidraw",
        "output must be an Excalidraw scene"
    );
}

// ---------------------------------------------------------------------------
// PowerPoint (.pptx) golden tests
// ---------------------------------------------------------------------------

fn compile_pptx_kzd(src: &str) -> Vec<u8> {
    let diagram = kozue_dsl::parse(src).expect("golden input must parse");
    let layout_out = kozue_layout::layout_full(&diagram).expect("golden layout must succeed");
    kozue_render_pptx::render(&layout_out.semantic).expect("golden pptx render must succeed")
}

const PPTX_GRAPH_GOLDEN_CASES: &[&str] = &[
    "chain",
    "branch",
    "skip",
    "node_shapes",
    "edge_presentation",
    "subgraph",
    "ports",
];
const PPTX_STATE_GOLDEN_CASES: &[&str] = &["state_basic", "state_bidirectional"];
const PPTX_SEQUENCE_GOLDEN_CASES: &[&str] = &[
    "seq_minimal",
    "seq_basic",
    "seq_self_dashed",
    "seq_participant_kinds",
    "seq_message_arrows",
    "seq_notes",
    "seq_dividers",
    "seq_activation",
];
const PPTX_CLASS_GOLDEN_CASES: &[&str] = &["class_basic"];
const PPTX_ER_GOLDEN_CASES: &[&str] = &["er_basic"];

fn run_pptx_golden_cases(cases: &[&str]) {
    for name in cases {
        let kzd = golden_dir().join(format!("{name}.kzd"));
        let pptx_path = golden_dir().join(format!("{name}.pptx"));
        let src =
            std::fs::read_to_string(&kzd).unwrap_or_else(|e| panic!("read {}: {e}", kzd.display()));
        let actual = compile_pptx_kzd(&src);

        if std::env::var("UPDATE_GOLDEN").is_ok() {
            std::fs::write(&pptx_path, &actual).unwrap();
            continue;
        }

        let expected = std::fs::read(&pptx_path).unwrap_or_else(|e| {
            panic!(
                "read golden {}: {e} (run with UPDATE_GOLDEN=1 to create it)",
                pptx_path.display()
            )
        });
        assert_eq!(
            actual, expected,
            "pptx golden mismatch for {name}.kzd (run with UPDATE_GOLDEN=1 to update)"
        );
    }
}

#[test]
fn pptx_graph_goldens_match() {
    run_pptx_golden_cases(PPTX_GRAPH_GOLDEN_CASES);
}

#[test]
fn pptx_state_goldens_match() {
    run_pptx_golden_cases(PPTX_STATE_GOLDEN_CASES);
}

#[test]
fn pptx_sequence_goldens_match() {
    run_pptx_golden_cases(PPTX_SEQUENCE_GOLDEN_CASES);
}

#[test]
fn pptx_class_er_goldens_match() {
    run_pptx_golden_cases(PPTX_CLASS_GOLDEN_CASES);
    run_pptx_golden_cases(PPTX_ER_GOLDEN_CASES);
}

#[test]
fn pptx_render_is_deterministic() {
    let src = std::fs::read_to_string(golden_dir().join("chain.kzd")).unwrap();
    let diagram = kozue_dsl::parse(&src).unwrap();
    let out1 = kozue_layout::layout_full(&diagram).unwrap();
    let out2 = kozue_layout::layout_full(&diagram).unwrap();
    let bytes1 = kozue_render_pptx::render(&out1.semantic).unwrap();
    let bytes2 = kozue_render_pptx::render(&out2.semantic).unwrap();
    assert_eq!(bytes1, bytes2, "pptx render must be deterministic");
}

/// Every pptx golden must be a well-formed ZIP (OPC) container: starts with a
/// local-file-header signature, contains an End-Of-Central-Directory
/// signature, and (since entries are stored uncompressed/STORE) the raw
/// slide1.xml text — including at least one shape and a label — appears
/// verbatim in the byte stream.
#[test]
fn pptx_goldens_are_well_formed_zip() {
    let cases = PPTX_GRAPH_GOLDEN_CASES
        .iter()
        .chain(PPTX_STATE_GOLDEN_CASES.iter())
        .chain(PPTX_SEQUENCE_GOLDEN_CASES.iter())
        .chain(PPTX_CLASS_GOLDEN_CASES.iter())
        .chain(PPTX_ER_GOLDEN_CASES.iter());
    for name in cases {
        let pptx_path = golden_dir().join(format!("{name}.pptx"));
        let bytes = std::fs::read(&pptx_path)
            .unwrap_or_else(|e| panic!("read golden {}: {e}", pptx_path.display()));
        assert!(
            bytes.starts_with(b"PK\x03\x04"),
            "{name}.pptx must start with a ZIP local file header signature"
        );
        assert!(
            bytes.windows(4).any(|w| w == b"PK\x05\x06"),
            "{name}.pptx must contain an End Of Central Directory signature"
        );
        assert!(
            bytes.windows(5).any(|w| w == b"<p:sp"),
            "{name}.pptx slide1.xml must contain at least one shape"
        );
    }
}

#[test]
fn pptx_cli_flag_produces_output() {
    let bin = env!("CARGO_BIN_EXE_kozue");
    let kzd = golden_dir().join("chain.kzd");
    let tmp_out = std::env::temp_dir().join("kozue_pptx_flag_test.pptx");
    let status = std::process::Command::new(bin)
        .args([
            "render",
            "--format",
            "pptx",
            kzd.to_str().unwrap(),
            "-o",
            tmp_out.to_str().unwrap(),
        ])
        .status()
        .expect("failed to run kozue");
    let content = std::fs::read(&tmp_out).unwrap_or_default();
    let _ = std::fs::remove_file(&tmp_out);
    assert!(status.success(), "render --format pptx should succeed");
    assert!(!content.is_empty(), "pptx output should be non-empty");
    assert!(
        content.starts_with(b"PK\x03\x04"),
        "output must be a ZIP (OPC) container"
    );
}
