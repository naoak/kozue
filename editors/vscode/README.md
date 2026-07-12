# kozue VSCode Extension

Provides real-time diagnostics (error squiggles) for kozue diagram files via
the [kozue-lsp](../../crates/kozue-lsp) language server.

## Supported file types

| Extension | Language |
|-----------|----------|
| `.kozue`, `.kzd` | Kozue DSL |
| `.mmd`, `.mermaid` | Mermaid |
| `.puml`, `.plantuml`, `.pu`, `.iuml` | PlantUML |

## Setup

1. **Build the language server:**
   ```sh
   cargo build -p kozue-lsp
   # Binary is at target/debug/kozue-lsp (or target/release/kozue-lsp for --release)
   ```

2. **Install extension dependencies:**
   ```sh
   cd editors/vscode
   npm install
   npm run compile
   ```

3. **Launch in Extension Development Host:**
   Open `editors/vscode/` in VSCode and press **F5**.

4. **Optional — set a custom server path:**
   Add to your VSCode `settings.json`:
   ```json
   { "kozue.serverPath": "/path/to/kozue-lsp" }
   ```
   By default the extension looks for `kozue-lsp` on your `PATH`.

## Scope (M6a)

M6a provides **diagnostics only** — parse errors are highlighted as you type.
Hover documentation, code formatting, go-to-definition, and other LSP features
are planned for M6b.
