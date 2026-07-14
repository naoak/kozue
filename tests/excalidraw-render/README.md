# Excalidraw render check

An **optional, cross-ecosystem** regression guard for the `kozue-render-excalidraw`
exporter. It renders every committed `tests/golden/*.excalidraw` file through
Excalidraw's *own* engine (`@excalidraw/excalidraw`'s `exportToSvg`, the same path
as excalidraw.com) and asserts they draw correctly.

This complements — it does not replace — the Rust byte-golden tests in
`crates/kozue-cli/tests/integration.rs`:

| Layer | Guards |
| --- | --- |
| Rust byte goldens (`cargo test`) | the exporter's output bytes don't change |
| This check (`npm test`) | the output actually *renders* in Excalidraw (no dropped labels, no broken bindings, non-degenerate scene) |

It is **not** wired into `cargo test`: `cargo` stays Node-independent. Run it
separately (locally or as its own CI job).

## Requirements

- Node.js ≥ 20 (uses the built-in `node:test` runner; no test framework dependency)

## Usage

```sh
cd tests/excalidraw-render
npm install      # first time only
npm test         # builds the Excalidraw bundle (pretest), then runs the checks
```

`npm run build` regenerates the bundle on its own if you need it.

## What is checked

For each golden, `exportToSvg` must:

1. **render without throwing** — a malformed element or a binding that points at a
   missing id makes the exporter throw;
2. **preserve every label** — each `text` element's string must appear in the
   rendered `<text>` output (catches dropped node/edge/message labels and
   container/arrow bound-text regressions);
3. **produce a non-degenerate viewBox** — nothing collapsed the scene.

Assertions are **semantic, not pixel snapshots**: rough.js draws with per-element
seeds and exact path data shifts between Excalidraw versions, so a byte snapshot
would be brittle without adding real coverage. See `verify.test.mjs`.

## Layout

| File | Role |
| --- | --- |
| `build.mjs` | esbuild-bundles `exportToSvg` into `dist/` (JSON imports inlined) |
| `dom-env.mjs` | jsdom + canvas / CSS-Font-Loading shims so the exporter runs headless |
| `render.mjs` | `loadGolden(name)` / `renderGolden(name)` helpers |
| `verify.test.mjs` | `node:test` cases asserting the invariants above |

`node_modules/`, the built `dist/` bundle, and any scratch `*.exc.svg` renders are
gitignored; the Excalidraw version is pinned in `package.json`.

> The headless environment cannot embed Excalidraw's hand-drawn web font (Virgil),
> so rendered text falls back to a system font. Element geometry, arrows and label
> **text** are identical to excalidraw.com; only the glyph shapes differ.
