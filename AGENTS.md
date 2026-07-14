# kozue

kozue is a deterministic diagram compiler written in Rust. It parses diagram
source (its own DSL, or Mermaid / PlantUML for compatibility) into a shared
semantic IR, lays the diagram out with a naive layered algorithm, and renders
it to one of several backends. **The same input always produces byte-identical
output** — this determinism is a hard requirement, not a nice-to-have, and the
golden tests exist to enforce it.

## Architecture

The workspace is a pipeline of small crates. Data flows left to right:

```
frontends → IR → layout → renderers
```

| Crate | Role |
| --- | --- |
| `kozue-ir` | The shared semantic IR (`Diagram`) that every frontend targets and every backend consumes. The contract between the two halves of the pipeline. |
| `kozue-dsl` | Frontend for the native `.kzd` DSL. Also owns `kozue fmt` (`format_kzd`). |
| `kozue-mermaid` | Compatibility frontend for Mermaid (`.mmd` / `.mermaid`). |
| `kozue-plantuml` | Compatibility frontend for PlantUML (`.puml` / `.plantuml` / `.pu` / `.iuml`). |
| `kozue-layout` | Layering, ordering, coordinate assignment. Produces a `Scene` (geometric) via `layout`, or a richer `LayoutOutput` / `SemanticLayout` via `layout_full` for exchange exporters. |
| `kozue-text` | Text measurement (embedded DejaVu Sans). Keeps box sizing deterministic. |
| `kozue-render-svg` | SVG backend (the default, canonical output). |
| `kozue-render-term` | Plain-text terminal backend. |
| `kozue-render-png` | Deterministic PNG rasterizer (tiny-skia). |
| `kozue-render-drawio` | draw.io / mxGraph XML exporter. Consumes the semantic layout, not the flat `Scene`. |
| `kozue-render-dot` | Graphviz DOT exporter. Consumes the semantic `Diagram` directly (no layout) — Graphviz lays out the graph itself. Graph & state diagrams only. |
| `kozue-cli` | The `kozue` binary. Wires frontends → layout → renderers. |
| `kozue-lsp` | Language server (diagnostics, hover, formatting). |
| `kozue-wasm` | WASM bindings for browser use. |

All three frontends expose `parse(source) -> Result<Diagram, Vec<Diagnostic>>`
(the DSL uses `Vec<CompileError>`). Because they converge on one IR, any layout
or renderer feature works for every input language automatically.

## CLI

```sh
kozue render examples/hello.kzd -o hello.svg     # default format is svg
kozue render input.mmd --format term             # svg | term | png | drawio | dot
kozue check examples/hello.kzd                    # parse + semantic check, prints OK
kozue fmt input.kzd --check                       # canonical form; --check for CI, --stdout to print
```

The frontend language is auto-detected from the file extension and can be
overridden with `--lang`. `fmt` is only supported for the native `.kzd` DSL.

## Build & test

```sh
cargo build                  # whole workspace
cargo build -p kozue-lsp     # a single crate
cargo test                   # all tests, including golden integration tests
```

Golden tests live in `tests/golden/` (driven by
`crates/kozue-cli/tests/integration.rs`): each `<name>.kzd` / `.mmd` / `.puml`
input has committed `.svg`, `.txt`, `.png`, `.drawio` outputs. When a change
legitimately alters output, regenerate them with:

```sh
UPDATE_GOLDEN=1 cargo test
```

Then **inspect the diff** before committing — an unexpected golden change is
usually a determinism bug, not a benign update.

## Text & Japanese glyphs

Widths are measured against embedded **DejaVu Sans**, which has no CJK glyphs.
Characters missing from the font fall back to an advance of `font_size`
(1 em/char), so layout stays deterministic even though the glyph actually shown
depends on the viewer's font fallback.

## Editor support

`kozue-lsp` provides real-time diagnostics, hover (shows a node/participant
label as Markdown), and document formatting for `.kzd` files. A VSCode
extension lives in `editors/vscode/`. See `README.md` for build details.

## Conventions

- **Determinism first.** No `HashMap` iteration order, timestamps, or
  randomness in anything that reaches output — use `IndexMap` / sorted order.
  If a golden changes unexpectedly, treat it as a regression.
- **Respect the boundaries.** Keep new work behind the
  frontend → IR → layout → renderer seams; a renderer should never reach back
  into a frontend, and frontends should only produce IR.
- **Commits** follow the milestone convention seen in the history, e.g.
  `M8c: sequence draw.io export`, with `fixup:`, `refactor:`, `layout:` etc. as
  scope-style prefixes. Keep the subject imperative and concise.
