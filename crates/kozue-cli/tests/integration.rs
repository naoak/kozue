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

const GOLDEN_CASES: &[&str] = &["chain", "branch", "right", "cycle", "skip", "wide_right"];

const SEQ_GOLDEN_CASES: &[&str] = &["seq_basic", "seq_self_dashed", "seq_minimal"];

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
    let src = "diagram d {\n a: \"A\"\n a -> ghost\n}";
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
    let src = "diagram seq {\n  participant a: \"A\"\n  a -> ghost : \"msg\"\n}";
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
    let src = "diagram seq {\n  participant a: \"A\"\n  participant a: \"B\"\n}";
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
    let src = "diagram seq {\n  participant a: \"A\"\n  b: \"B\"\n}";
    let result = kozue_dsl::parse(src);
    assert!(result.is_err());
    let errs = result.unwrap_err();
    assert!(
        errs.iter().any(|e| e.message.contains("mix")),
        "error should mention mixing, got: {errs:?}"
    );
}

#[test]
fn dashed_edge_in_graph_is_error() {
    let src = "diagram d {\n  a: \"A\"\n  b: \"B\"\n  a --> b\n}";
    let result = kozue_dsl::parse(src);
    assert!(
        result.is_err(),
        "dashed edge in graph diagram must be an error"
    );
}

#[test]
fn seq_long_label_widens_columns() {
    let src = r#"diagram seq {
  participant a: "A"
  participant b: "B"
  a -> b : "this is a very long message label that should widen the columns"
}"#;
    let diagram = kozue_dsl::parse(src).expect("should parse");
    let scene = kozue_layout::layout(&diagram).expect("should layout");

    let src_short = r#"diagram seq {
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

const MERMAID_GOLDEN_CASES: &[&str] = &["mermaid_flow", "mermaid_seq", "mermaid_state"];

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

const PLANTUML_GOLDEN_CASES: &[&str] = &["plantuml_seq", "plantuml_state"];

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
const MINIMAL_KZD: &str = "diagram d {\n  a: \"A\"\n  b: \"B\"\n  a -> b\n}\n";

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

const CANONICAL_KZD: &str = "diagram d {\n  a: \"A\"\n  b: \"B\"\n\n  a -> b\n}\n";
const UNFORMATTED_KZD: &str = "diagram d{a:\"A\"\nb:\"B\"\na->b}\n";

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
    let bad_src = "diagram d { bad syntax !!! }\n";
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

const TERM_GOLDEN_KZD_CASES: &[&str] = &["chain", "branch", "seq_basic"];
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

const PNG_GOLDEN_CASES: &[&str] = &["chain", "branch", "seq_basic"];

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
