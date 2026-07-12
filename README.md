# kozue

kozue is a diagram compiler that takes a custom DSL (`.kzd`) as input and produces deterministic SVG output. It parses the DSL into a semantic IR, lays out the diagram with a naive layered algorithm, and renders it to SVG. The same input always produces byte-identical output.

## Usage

```sh
kozue render examples/hello.kzd -o hello.svg
kozue check examples/hello.kzd
```

`render` compiles a diagram to SVG (defaults to `<input>.svg` if `-o` is omitted). `check` parses and semantically validates the input, printing `OK` on success. Parse and semantic errors are printed to stderr with a non-zero exit code.

## Note on Japanese glyphs

Text width is measured with the embedded **DejaVu Sans** font. DejaVu Sans does not contain Japanese glyphs, so for any character missing from the font (such as kanji, hiragana, and katakana) a fallback advance width of `font_size` (1 em per character) is used. Labels still render as text in the SVG with `font-family="DejaVu Sans"`; the actual glyph shown depends on the viewer's font fallback, but layout box sizes remain deterministic.

## Editor support (LSP)

kozue ships a Language Server Protocol implementation (`kozue-lsp`) that provides real-time parse diagnostics (error squiggles) for `.kozue`/`.kzd`, `.mmd`/`.mermaid`, and `.puml`/`.plantuml`/`.pu`/`.iuml` files.

### Build the language server

```sh
cargo build -p kozue-lsp
# Binary: target/debug/kozue-lsp  (or target/release/kozue-lsp with --release)
```

### VSCode extension

A ready-made VSCode extension lives in [`editors/vscode/`](editors/vscode/). It launches `kozue-lsp` over stdio and forwards diagnostics to the Problems panel.

```sh
cd editors/vscode
npm install
npm run compile
# Then open editors/vscode/ in VSCode and press F5 to launch the Extension Development Host.
```

To use a custom binary path, set `"kozue.serverPath"` in your VSCode `settings.json`.

### Scope (M6b)

The LSP server currently provides:

- **Diagnostics** — parse errors appear as squiggles in real time (M6a).
- **Hover** — hovering over a node or participant id shows its label as Markdown (M6b). Works for all supported languages (kozue, Mermaid, PlantUML), since they share one IR.
- **Formatting** — running "Format Document" on a `.kozue`/`.kzd` file reformats it with `kozue fmt` (M6b). Mermaid and PlantUML files are left untouched (no formatter exists for them yet).

Go-to-definition and other features are planned for future milestones.
