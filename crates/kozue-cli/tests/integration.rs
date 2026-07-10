//! Integration tests for the kozue pipeline.
//!
//! - Golden tests: each `tests/golden/*.kzd` must render to the committed
//!   `*.svg` byte-for-byte.
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
